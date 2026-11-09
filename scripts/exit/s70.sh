#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Live progress for test and command steps (s70) exit test, against a real masond.
#  1. A test step running a TAP suite of 40 tests: after a first green run (40 tests), a second
#     run on a new commit that prints a test every 0.15 s shows its passed count RISING over the
#     WebSocket, against a total of 40 from the last green run (source "expected"). The final
#     verdict is the full parse (the task goes on to land).
#  2. A suite that appends to its `progress` file shows those counts and messages.
#  3. A command step writing MASON_PROGRESS_FILE shows its counts and message.
# (The plugin protocol 2 and the CI check counts are covered by cargo tests, not here.)
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
command -v python3 >/dev/null || { echo "s70 needs python3 (its WebSocket client)" >&2; exit 1; }
root=$(mktemp -d /tmp/mason-s70.XXXXXX); daemon=""; watcher=""
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
ws=scripts/exit/fixtures/s70/wsclient.py
owner=${USER:-owner}
port=$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1])')
m() { "$bin/mason" --data-dir "$root/d" --config-dir "$root/c" "$@"; }
fail() { set +e; echo "FAILED: $*" >&2; m task list >&2 2>&1; tail -20 "$root/masond.log" >&2; exit 1; }
wait_for() {  # SECONDS DESCRIPTION COMMAND...
  local end=$((SECONDS + $1)); local what=$2; shift 2
  until "$@" >/dev/null 2>&1; do [ $SECONDS -lt $end ] || fail "timed out waiting for: $what"; sleep 0.2; done
}
rm -rf "$root/d" "$root/c"; mkdir -p "$root/d" "$root/c/projects"
git init -q --bare -b main "$root/origin.git"
git clone -q "$root/origin.git" "$root/repo" 2>/dev/null
git -C "$root/repo" checkout -q -b main
mkdir -p "$root/repo/.mason"
cat > "$root/repo/.mason/mason.toml" <<'TOML'
schema = 1
start = "work"

[step.work]
type = "command"
category = "developing"
command = '''
echo "$MASON_TITLE" > "change-$MASON_TASK.txt"
git add -A
git -c user.name=t -c user.email=t@t commit -qm "$MASON_TITLE"
'''
next = "test"

[step.test]
type = "test"
category = "testing"
suites = ["tap40"]
next = "ptest"

[step.ptest]
type = "test"
category = "testing"
suites = ["pf"]
next = "script"

[step.script]
type = "command"
category = "testing"
command = '''
for i in 1 2 3 4; do
  echo "{\"done\": $i, \"total\": 4, \"message\": \"stage $i\"}" >> "$MASON_PROGRESS_FILE"
  sleep 0.5
done
echo "no json here" >> "$MASON_PROGRESS_FILE"
sleep 1
'''
next = "land"

[step.land]
type = "landing"
category = "landing"

[suite.tap40]
command = "sh tap40.sh"
parser = "tap"
min_tests = 3
level = "unit"

[suite.pf]
command = "sh pf.sh"
parser = "tap"
min_tests = 1
level = "unit"
progress = "pf.jsonl"
TOML
printf 'mode = "direct"\n' > "$root/repo/.mason/landing.toml"
cat > "$root/repo/tap40.sh" <<SH
echo 1..40
i=1
while [ \$i -le 40 ]; do
  echo "ok \$i - case \$i"
  [ -f "$root/slow.flag" ] && sleep 0.15
  i=\$((i+1))
done
SH
cat > "$root/repo/pf.sh" <<'SH'
#!/bin/sh
echo '{"done": 1, "total": 3, "message": "warming up"}' >> pf.jsonl
sleep 1
echo 'plain words, not json' >> pf.jsonl
sleep 1
echo '{"done": 3, "total": 3}' >> pf.jsonl
sleep 1
echo 1..2; echo "ok 1 - a"; echo "ok 2 - b"
SH
git -C "$root/repo" add -A
git -C "$root/repo" -c user.name=t -c user.email=t@t commit -qm pipeline
git -C "$root/repo" push -q origin main
cat > "$root/c/projects/web.toml" <<TOML
path = "$root/repo"
base = "main"
remote = "origin"
test = "true"
TOML
"$bin/masond" --data-dir "$root/d" --config-dir "$root/c" --port "$port" --max-concurrent 2 --heartbeat-ms 100 --sweep-ms 3600000 --owner "$owner" 2>>"$root/masond.log" &
daemon=$!
until m status >/dev/null 2>&1; do kill -0 "$daemon" || { cat "$root/masond.log" >&2; exit 1; }; sleep 0.1; done
add() { m task add "$1" --project web --body "$1" | sed -n 's/^Filed task #\([0-9]*\) .*/\1/p'; }
state_of() { m task show "$1" --json | sed -n 's/^  "state": "\(.*\)",$/\1/p'; }
finished() { case "$(state_of "$1")" in done|landed|merged|failed|stopped) return 0 ;; *) return 1 ;; esac; }

echo "== first: a quick green run records 40 tests for this base"
t1=$(add "first change")
wait_for 120 "task $t1 to finish" finished "$t1"
echo "   task $t1: $(state_of "$t1")"

echo "== second: a slow run on a new commit, watched over the WebSocket"
touch "$root/slow.flag"
t2=$(add "second change")
python3 "$ws" watch "$port" "$t2" 90 "$root/ws.jsonl" &
watcher=$!
wait_for 150 "task $t2 to finish" finished "$t2"
sleep 1
kill "$watcher" 2>/dev/null || true; watcher=""
echo "   task $t2: $(state_of "$t2")"
python3 - "$root/ws.jsonl" <<'PY'
import json, sys
recs = [json.loads(l) for l in open(sys.argv[1])]
def of(step): return [r for r in recs if r["step"] == step]
t = of("test")
assert t, "no StepProgress for the test step"
dones = [r["done"] for r in t if r["unit"] == "tests"]
rising = sorted(set(dones))
assert len(rising) >= 4 and rising == sorted(rising) and rising[-1] == 40, "the passed count did not rise: %r" % dones
assert all(r["total"] == 40 and r["total_source"] == "expected" for r in t if r["unit"] == "tests"), t[-1]
mid = [r for r in t if 0 < r["done"] < 40]
assert mid, "no push between 0 and 40 tests"
assert mid[0]["subs"][0]["name"] == "tap40" and mid[0]["subs"][0]["total"] == 40 and mid[0]["subs"][0]["done_source"] == "counted", mid[0]
print("   test step: passed rose through %d distinct counts to %d of %d (%s)" % (len(rising), rising[-1], t[-1]["total"], t[-1]["total_source"]))
p = of("ptest")
msgs = [r["message"] for r in p]
assert any("warming up" in x for x in msgs) and any("plain words, not json" in x for x in msgs), msgs
assert any(r["subs"] and r["subs"][0]["done"] == 1 and r["subs"][0]["total"] == 3 and r["subs"][0]["done_source"] == "reported" for r in p), p
assert any(r["subs"] and r["subs"][0]["done"] == 3 for r in p), p
print("   ptest step: the suite's progress file gave %r" % [x for x in dict.fromkeys(msgs) if x])
s = of("script")
sm = [r["message"] for r in s]
assert any(r["done"] == 2 and r["total"] == 4 and r["done_source"] == "reported" for r in s), s
assert "stage 3" in sm and "no json here" in sm, sm
print("   script step: MASON_PROGRESS_FILE gave done 1..4 of 4 and messages %r" % [x for x in dict.fromkeys(sm) if x])
PY
echo "s70 passed"
