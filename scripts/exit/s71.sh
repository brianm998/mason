#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Agent steps report real progress (s71) exit test: a real masond, a scripted agent (fake-claude).
#  a. A develop step calls report_progress with 1 of 4, then 2 of 4: each shows on the WebSocket
#     within a second of the call, with its message, unit and a reported source.
#  b. A scripted TodoWrite list (3 of 7 done, one in progress) shows as "plan steps": 3 of 7, the
#     item in progress as the message.
#  c. An agent that reports nothing shows tool calls (counted) with the latest tool line as the
#     message, and output lines; with fewer than five successful runs there is no total, and the
#     sixth run shows a `history` total (never a reported one).
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
command -v python3 >/dev/null || { echo "s71 needs python3 (its WebSocket client)" >&2; exit 1; }
root=$(mktemp -d /tmp/mason-s71.XXXXXX); daemon=""; watcher=""
cleanup() {
  [ -n "$watcher" ] && kill "$watcher" 2>/dev/null || true
  [ -n "$daemon" ] && kill -9 "$daemon" 2>/dev/null || true
  for f in "$root"/d/h/*/host.kv; do
    [ -e "$f" ] || continue
    hp=$(sed -n "s/^host_pid=//p" "$f"); cp=$(sed -n "s/^child_pid=//p" "$f"); if [ -n "$hp" ] && ps -o command= -p "$hp" 2>/dev/null | grep -qF "step-host --dir $(dirname "$f")"; then kill -9 "-$cp" 2>/dev/null || true; kill -9 "$hp" 2>/dev/null || true; fi  # only a live step host: a stale pid may belong to someone else now
  done
  rm -rf "$root"
}
trap cleanup EXIT
cargo build -q --workspace --bins
bin=target/debug
ws=scripts/exit/fixtures/s71/wsclient.py
owner=${USER:-owner}
port=$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1])')
m() { "$bin/mason" --data-dir "$root/d" --config-dir "$root/c" "$@"; }
fail() { echo "FAILED: $*" >&2; m task list >&2 2>&1 || true; tail -20 "$root/masond.log" >&2 || true; exit 1; }
wait_for() {  # SECONDS DESCRIPTION COMMAND...
  local end=$((SECONDS + $1)); local what=$2; shift 2
  until "$@" >/dev/null 2>&1; do [ $SECONDS -lt $end ] || fail "timed out waiting for: $what"; sleep 0.3; done
}
developed() { [ "$(m task show "$1" | grep -ci ' develop .*succeeded')" -ge 1 ]; }
add() { m task add "$2" --project "$1" --explored --body "x" | sed -n 's/^Filed task #\([0-9]*\) .*/\1/p'; }

# The scripted agent: a few lines of JSON per project. NOW stamps the clock (ms) in calls.log.
now='{"sh": "python3 -c \"import time; print(int(time.time()*1000))\" >> '$root'/calls.log"}'
commit='{"sh": "echo x >> f.txt; git add -A && git -c user.name=t -c user.email=t@t commit -qm change"}'
submit='{"call": {"tool": "submit_change", "args": {"summary": "done", "head": "$HEAD"}}}'
rep() { echo "{\"call\": {\"tool\": \"report_progress\", \"args\": {\"message\": \"$1\", \"done\": $2, \"total\": $3, \"unit\": \"files\"}}}"; }
todo() {  # three done, one doing, three pending
  echo '{"todo": [{"content": "Read the code", "status": "completed"}, {"content": "Write the test", "status": "completed"}, {"content": "Add the type", "status": "completed"}, {"content": "Wire the daemon", "status": "in_progress"}, {"content": "Run the suite", "status": "pending"}, {"content": "Write the note", "status": "pending"}, {"content": "Commit", "status": "pending"}]}'
}
tool() { echo "{\"tool\": {\"name\": \"$1\", \"input\": $2}}"; }
sleep_ms() { echo "{\"sleep_ms\": $1}"; }

mkproject() {  # NAME TURN-ACTIONS
  local n=$1
  git init -q --bare -b main "$root/$n.git"
  git clone -q "$root/$n.git" "$root/$n" 2>/dev/null
  git -C "$root/$n" checkout -q -b main
  echo base > "$root/$n/base.txt"
  git -C "$root/$n" add -A
  git -C "$root/$n" -c user.name=t -c user.email=t@t commit -qm init
  git -C "$root/$n" push -q origin main
  echo "{\"steps\": {\"develop\": [[$2]]}}" > "$root/$n.json"
  cat > "$root/c/projects/$n.toml" <<TOML
path = "$root/$n"
base = "main"
remote = "origin"
test = "true"
claude_bin = "$PWD/$bin/fake-claude"
[agent_env]
FAKE_CLAUDE_SCRIPT = "$root/$n.json"
TOML
}

mkdir -p "$root/c/projects"
mkproject pa "$now, $(rep 'reading the files' 1 4), $(sleep_ms 2500), $now, $(rep 'editing the files' 2 4), $(sleep_ms 2500), $commit, $submit"
mkproject pb "$(todo), $(sleep_ms 3000), $commit, $submit"
mkproject pc "$(tool Read '{"file_path": "crates/daemon/src/views.rs"}'), $(sleep_ms 1300), $(tool Edit '{"file_path": "crates/daemon/src/views.rs"}'), $(sleep_ms 1300), $(tool Bash '{"command": "cargo test API_TOKEN=hunter2"}'), $(sleep_ms 2000), $commit, $submit"
"$bin/masond" --data-dir "$root/d" --config-dir "$root/c" --port "$port" --max-concurrent 6 --heartbeat-ms 100 --sweep-ms 3600000 --owner "$owner" 2>>"$root/masond.log" &
daemon=$!
until m status >/dev/null 2>&1; do kill -0 "$daemon" || { cat "$root/masond.log" >&2; exit 1; }; sleep 0.1; done
python3 "$ws" watch "$port" 0 240 "$root/ws.jsonl" &
watcher=$!
sleep 1

echo "== a. report_progress 1 of 4, then 2 of 4"
ta=$(add pa "report counts")
wait_for 90 "the reporting step to finish" developed "$ta"
echo "== b. a TodoWrite list"
tb=$(add pb "keep a todo list")
wait_for 90 "the todo step to finish" developed "$tb"
echo "== c. an agent that reports nothing: five runs, then a sixth with history"
first=$(add pc "silent one")
wait_for 90 "the first silent step to finish" developed "$first"
for i in 2 3 4 5; do t=$(add pc "silent $i"); wait_for 90 "silent $i to finish" developed "$t"; done
sixth=$(add pc "silent six")
wait_for 90 "the sixth silent step to finish" developed "$sixth"
sleep 2
kill "$watcher" 2>/dev/null || true; watcher=""
python3 - "$root/ws.jsonl" "$root/calls.log" "$ta" "$tb" "$first" "$sixth" <<'PY'
import json, sys
recs = [json.loads(l) for l in open(sys.argv[1])]
calls = [int(x) for x in open(sys.argv[2]).read().split()]
ta, tb, first, sixth = (int(x) for x in sys.argv[3:7])
dev = lambda t: [r for r in recs if r["task"] == t and r["step"] == "develop"]

# a. each report on the socket within a second of the call, with its message
assert len(calls) == 2, f"expected two stamped calls, got {calls}"
for (msg, d, tot), at in zip([("reading the files", 1, 4), ("editing the files", 2, 4)], calls):
    hit = [r for r in dev(ta) if r["message"] == msg]
    assert hit, f"no push carried {msg!r}: {[r['message'] for r in dev(ta)]}"
    r = hit[0]
    assert (r["done"], r["total"], r["unit"]) == (d, tot, "files"), r
    assert r["done_source"] == "SOURCE_REPORTED" and r["total_source"] == "SOURCE_REPORTED", r
    lag = r["t"] - at
    assert -50 <= lag <= 1000, f"{d} of {tot} reached the socket {lag} ms after the call"
    print(f"   {d} of {tot} files, {msg!r}: on the socket {lag} ms after the call")
# the first report was never replaced by a count of tool calls or lines
assert not any(r["unit"] in ("lines", "tool calls") and r["done_source"] != "SOURCE_REPORTED" and r["message"] == "editing the files" for r in dev(ta))

# b. the todo list: plan steps, 3 of 7, the item in progress as the message
hit = [r for r in dev(tb) if r["unit"] == "plan steps"]
assert hit, f"no plan steps push: {dev(tb)}"
r = hit[0]
assert (r["done"], r["total"], r["done_source"], r["message"]) == (3, 7, "SOURCE_REPORTED", "Wire the daemon"), r
assert len(r["todo"]) == 7, r["todo"]
print(f"   plan steps {r['done']} of {r['total']}, now: {r['message']!r}")

# c. nothing reported: tool calls and lines; history only from the sixth run
def silent(t):
    rs = [r for r in dev(t) if r["unit"] == "tool calls"]
    assert rs, f"no tool-call push for task {t}: {dev(t)}"
    return rs
a = silent(first)
assert all(r["done_source"] == "SOURCE_COUNTED" for r in a)
assert not any(r["total_source"] == "SOURCE_HISTORY" for r in dev(first)), "the first run had history"
assert max(r["done"] for r in dev(first)) >= 3, [r["done"] for r in a]
b = silent(sixth)
assert any(r["total_source"] == "SOURCE_HISTORY" and r["total"] > 0 and r["history_samples"] >= 5 for r in b), b
assert not any(r["total_source"] in ("SOURCE_REPORTED",) for r in b), "a typical figure is never a reported one"
msgs = [r["message"] for r in b]
assert any(m.startswith("Reading ") for m in msgs) and any(m.startswith("Editing ") for m in msgs), msgs
assert any(m.startswith("Running cargo test") for m in msgs), msgs
assert not any("hunter2" in m for r in recs for m in [r["message"]]), "a secret reached the socket"
assert any("API_TOKEN=[secret]" in m for m in msgs), msgs
assert max(r["lines"] for r in b) > 0, "output lines are counted too"
last = [r for r in b if r["total_source"] == "SOURCE_HISTORY"][-1]
print(f"   silent: {last['done']} tool calls, typical {last['total']} (history of {last['history_samples']}); lines {max(r['lines'] for r in b)}; now: {msgs[-1]!r}")
PY
echo "s71 passed"
