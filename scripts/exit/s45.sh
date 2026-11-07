#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Live step progress (s45) exit test: a real masond, a pipeline whose first step is a command.
#  1. A command step prints a line every 200 ms for 10 s: a WebSocket client sees StepProgress
#     pushes while it runs, no more than about one a second, each with more lines than the one
#     before and a later last_output_ms; the task's view carries the same figures.
#  2. The daemon is killed part way through (the step host lives on) and started again: the line
#     count carries on from where it was and ends at exactly 50, with nothing counted twice.
#  3. A step that prints nothing for 5 s keeps the same updated_at and last output.
#  4. With fewer than five successful runs of a step there is no typical total; after five, the
#     sixth run shows a `history` total (never a reported one).
#  5. GET /tasks/{id}/progress (mason task progress) shows the same record.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
command -v python3 >/dev/null || { echo "s45 needs python3 (its WebSocket client)" >&2; exit 1; }
root=$(mktemp -d /tmp/mason-s45.XXXXXX); daemon=""; watcher=""
cleanup() {
  [ -n "$watcher" ] && kill "$watcher" 2>/dev/null || true
  [ -n "$daemon" ] && kill -9 "$daemon" 2>/dev/null || true
  for f in "$root"/d/h/*/host.kv; do
    [ -e "$f" ] || continue
    for k in host_pid child_pid; do pid=$(sed -n "s/^$k=//p" "$f"); kill -9 "-$pid" 2>/dev/null || kill -9 "$pid" 2>/dev/null || true; done
  done
  rm -rf "$root"
}
trap cleanup EXIT
cargo build -q --workspace --bins
bin=target/debug
ws=scripts/exit/fixtures/s45/wsclient.py
owner=${USER:-owner}
port=$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1])')
m() { "$bin/mason" --data-dir "$root/d" --config-dir "$root/c" "$@"; }
fail() { echo "FAILED: $*" >&2; m task list >&2 2>&1 || true; tail -20 "$root/masond.log" >&2 || true; exit 1; }
state_of() { m task show "$1" --json 2>/dev/null | sed -n 's/^  "state": "\(.*\)",$/\1/p'; }
wait_for() {  # SECONDS DESCRIPTION COMMAND...
  local end=$((SECONDS + $1)); local what=$2; shift 2
  until "$@" >/dev/null 2>&1; do [ $SECONDS -lt $end ] || fail "timed out waiting for: $what"; sleep 0.2; done
}
start_daemon() {
  "$bin/masond" --data-dir "$root/d" --config-dir "$root/c" --port "$port" --max-concurrent 4 --heartbeat-ms 100 --sweep-ms 3600000 --owner "$owner" 2>>"$root/masond.log" &
  daemon=$!
  until m status >/dev/null 2>&1; do kill -0 "$daemon" || { cat "$root/masond.log" >&2; exit 1; }; sleep 0.1; done
}
add() { m task add "$1" --project web --body "$1" | sed -n 's/^Filed task #\([0-9]*\) .*/\1/p'; }
prog() { m task progress "$1" --json; }  # JSON of the open runs
# FIELD of the first record of `mason task progress --json`
pf() { prog "$1" | python3 -c "import json,sys; r=json.load(sys.stdin)['runs']; d=r[0] if r else None; print(eval(sys.argv[1], {'d': d}) if d else '')" "$2"; }
ge() { [ "$(pf "$1" "$2")" -ge "$3" ] 2>/dev/null; }  # TASK EXPR N: the figure is at least N
worked() { [ "$(m task show "$1" | grep -ci ' work .*succeeded')" -ge 1 ]; }  # the work step succeeded

echo "== setup: a bare repository and a pipeline whose first step is a command (its behaviour follows the task's title)"
git init -q --bare -b main "$root/origin.git"
git clone -q "$root/origin.git" "$root/repo" 2>/dev/null
git -C "$root/repo" checkout -q -b main
mkdir -p "$root/repo/.mason" "$root/c/projects"
cat > "$root/repo/.mason/mason.toml" <<'TOML'
schema = 1
start = "work"

[step.work]
type = "command"
category = "developing"
command = '''
case "$MASON_TITLE" in
  slow*) i=0; while [ $i -lt 50 ]; do echo "line $i"; i=$((i+1)); sleep 0.2; done ;;
  silent*) echo start; sleep 5; echo end ;;
  held*) echo one; sleep 600 ;;
  *) echo a; echo b; echo c ;;
esac
'''
next = "hold"

[step.hold]
type = "command"
category = "testing"
command = "sleep 600"
next = "land"

[step.land]
type = "landing"
category = "landing"
TOML
printf 'mode = "direct"\n' > "$root/repo/.mason/landing.toml"
echo base > "$root/repo/base.txt"
git -C "$root/repo" add -A
git -C "$root/repo" -c user.name=t -c user.email=t@t commit -qm pipeline
git -C "$root/repo" push -q origin main
cat > "$root/c/projects/web.toml" <<TOML
path = "$root/repo"
base = "main"
remote = "origin"
test = "true"
TOML
start_daemon

echo "== 1. a step printing every 200 ms for 10 s, watched over the WebSocket"
python3 "$ws" watch "$port" 0 30 "$root/ws.jsonl" &
watcher=$!
t1=$(add "slow one")
wait_for 20 "the slow step to start counting" ge "$t1" "d['output']['lines']" 5
sleep 3
snap=$(python3 "$ws" snap "$port" "$t1")
echo "   a view of the task mid-run: $snap"
before=$(pf "$t1" "d['output']['lines']")
echo "   $before lines so far; killing the daemon (the step host keeps the command running)"
kill -9 "$daemon"; wait "$daemon" 2>/dev/null || true; daemon=""
sleep 1
start_daemon
wait_for 20 "the record after the restart" ge "$t1" "d['output']['lines']" "$before"
after=$(pf "$t1" "d['output']['lines']")
echo "   after the restart: $after lines (was $before)"
[ "$after" -ge "$before" ] || fail "the count went backwards over the restart: $before then $after"
wait_for 40 "the slow step to finish" worked "$t1"
sleep 2
kill "$watcher" 2>/dev/null || true; watcher=""
python3 - "$root/ws.jsonl" "$snap" <<'PY'
import json, sys
recs = [json.loads(l) for l in open(sys.argv[1])]
snap = json.loads(sys.argv[2])
steps = [r for r in recs if r["kind"] == "step" and r["step"] == "work"]
live = [r for r in steps if r["state"] == "PART"]
assert len(live) >= 5, f"only {len(live)} StepProgress pushes while it ran"
for a, b in zip(live, live[1:]):
    if b["now_ms"] - a["now_ms"] < 900:
        # a push at once for a state change is allowed; output alone is not
        assert False, f"pushes {b['now_ms'] - a['now_ms']} ms apart: {a} {b}"
    assert b["lines"] > a["lines"], f"lines did not rise: {a['lines']} then {b['lines']}"
    assert b["last_output_ms"] > a["last_output_ms"], "last_output_ms did not move"
    assert b["bytes"] > a["bytes"], "bytes did not rise"
assert all(r["unit"] == "lines" and r["done_source"] == "SOURCE_COUNTED" and r["done"] == r["lines"] for r in live), "the generic count is lines, counted"
final = steps[-1]
assert final["state"] == "FILLED" and final["lines"] == 50, f"the last push should be the final count of 50 lines: {final}"
assert any(r["kind"] == "welcome" and r["now_ms"] > 0 for r in recs), "Welcome carries now_ms"
# the task's view carries the same figures: it sits between the pushes around it
assert snap and snap["unit"] == "lines" and snap["done_source"] == "SOURCE_COUNTED", f"the view's cell: {snap}"
lo = max([r["lines"] for r in live if r["last_output_ms"] <= snap["last_output_ms"]] or [0])
hi = min([r["lines"] for r in live if r["last_output_ms"] >= snap["last_output_ms"]] or [10**9])
assert lo <= snap["lines"] <= hi, f"the view's {snap['lines']} lines is not between the pushes' {lo} and {hi}"
assert snap["done"] == snap["lines"] and snap["run"] == live[0]["run"]
gaps = [b["now_ms"] - a["now_ms"] for a, b in zip(live, live[1:])]
print(f"   {len(live)} pushes while it ran, {min(gaps)}..{max(gaps)} ms apart; lines {live[0]['lines']}..{live[-1]['lines']}, then a final 50")
PY
echo "   the count carried on over the restart and ended at exactly 50 lines"

echo "== 3. a step that prints nothing for 5 s keeps its updated_at"
t2=$(add "silent two")
wait_for 20 "the silent step to start" ge "$t2" "d['output']['lines']" 1
u1=$(pf "$t2" "d['updated_at_ms']"); l1=$(pf "$t2" "d['output']['last_output_ms']")
sleep 3.5
u2=$(pf "$t2" "d['updated_at_ms']"); l2=$(pf "$t2" "d['output']['last_output_ms']")
echo "   updated_at $u1 then $u2, last output $l1 then $l2, 3.5 s apart"
[ "$u1" = "$u2" ] && [ "$l1" = "$l2" ] || fail "a silent step's record moved: $u1 $u2 $l1 $l2"
prog "$t2" | grep -q '"lines": 1' || fail "the record does not say 1 line"
m task progress "$t2" | grep -q "1 lines" || fail "mason task progress does not show the lines: $(m task progress "$t2")"
wait_for 20 "the silent step to finish" worked "$t2"

echo "== 4. history: no typical total before five successful runs, a history total after"
for n in 1 2; do q=$(add "quick $n"); wait_for 20 "quick $n" worked "$q"; done
h1=$(add "held one")
wait_for 20 "held one to run" ge "$h1" "d['output']['lines']" 1
[ "$(pf "$h1" "d['total']['source']")" = none ] || fail "a total with only four successful runs: $(prog "$h1")"
echo "   four successful runs: total source none"
m task cancel "$h1" >/dev/null
q=$(add "quick 3"); wait_for 20 "quick 3" worked "$q"
h2=$(add "held two")
wait_for 20 "held two to run" ge "$h2" "d['output']['lines']" 1
[ "$(pf "$h2" "d['total']['source']")" = history ] || fail "no history total after five successful runs: $(prog "$h2")"
tot=$(pf "$h2" "d['total']['value']")
[ "$tot" = 3 ] || fail "the typical figure should be the high end of the middle half (3): $tot"
cell=$(python3 "$ws" snap "$port" "$h2")
echo "   five successful runs: total $tot, source history; the view's cell says $(echo "$cell" | python3 -c 'import json,sys; c=json.load(sys.stdin); print(c["total"], c["total_source"], c["done_source"])')"
echo "$cell" | python3 -c 'import json,sys; c=json.load(sys.stdin); assert c["total_source"]=="SOURCE_HISTORY" and c["total"]==3 and c["done_source"]=="SOURCE_COUNTED", c' || fail "the view's cell lacks the history total: $cell"
m task cancel "$h2" >/dev/null
echo "s45 OK"
