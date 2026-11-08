#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Live step progress shown (s49) exit test.
#  1. Against `masond --demo`, the daemon's own reading of each case (real count, typical total,
#     no total, quiet, sub-counts, todo list) has the right words: a percentage only from a real
#     total, the count and range with no percentage for a typical one, the count alone for none,
#     "silent" for the quiet step.
#  2. Both monitors (Compose, and SwiftUI where Swift is present) show exactly those items, and a
#     planted difference in either is caught.
#  3. Against a real masond with a command step printing every 200 ms: `mason task show --json`
#     during the run carries the same record as the WebSocket StepProgress; `mason task show`
#     prints the progress line; `mason task show --watch` and `mason status --watch` follow it.
# The Swift half needs macOS; the Compose half needs Java and Gradle (MASON_SKIP_COMPOSE skips it).
# The pixels (cell fill, amber, the counting time) are covered by the clients' unit tests, not here.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
command -v python3 >/dev/null || { echo "s49 needs python3 (its WebSocket client)" >&2; exit 1; }
root=$(mktemp -d /tmp/mason-s49.XXXXXX); daemon=""; watcher=""; w2=""; w3=""
cleanup() {
  for p in "$watcher" "$w2" "$w3"; do [ -n "$p" ] && kill "$p" 2>/dev/null || true; done
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
port_of() { sed -n 's/.*"port": *\([0-9]*\).*/\1/p' "$root/d/daemon.json"; }
start_demo() {
  rm -rf "$root/d"; mkdir -p "$root/d"
  "$bin/masond" --demo-manual --data-dir "$root/d" --config-dir "$root/c" 2>>"$root/masond.log" &
  daemon=$!
  until [ -s "$root/d/daemon.json" ]; do kill -0 "$daemon" || { cat "$root/masond.log" >&2; exit 1; }; sleep 0.1; done
}
stop_demo() { if kill -9 "$daemon" 2>/dev/null; then wait "$daemon" 2>/dev/null; fi; daemon=""; }

echo "== 1. the demo's six cases, as the daemon's views say them"
start_demo; "$bin/conformance" expected --port "$(port_of)" --out "$root/expected" | tail -1; stop_demo
python3 - "$root/expected" <<'PY'
import json, sys, glob
files = sorted(glob.glob(sys.argv[1] + "/checkpoint-*.json"))
items = {i["id"]: i["texts"] for i in json.load(open(files[-1]))}
def want(task, first, has=(), lacks=()):
    t = items["progress:%d:develop" % task]
    assert t[0] == first, (task, t)
    for h in has: assert h in t, (task, h, t)
    for l in lacks: assert l not in t, (task, l, t)
    return t
want(5, "212 of 340 tests, 62%", has=["running the api suite", "count from the agent"], lacks=["silent"])
t = want(6, "1,240 lines, usually 1,800 to 2,400", has=["total usually, from 12 past runs"], lacks=["silent"])
assert not any("%" in x for x in t), t
t = want(7, "37 tool calls", lacks=["silent"])
assert not any("%" in x or " of " in x for x in t if x == t[0]), t
want(8, "412 lines", has=["silent"])
want(9, "3 of 7 plan steps, 42%", has=["unit: 120 of 120 tests, 100%", "integration: 44 of 90 tests, 48%"], lacks=["silent"])
want(10, "1 of 3 todo items, 33%", has=["done: Read the schema", "doing: Write the migration", "pending: Add a rollback test"], lacks=["silent"])
for i, t in items.items():
    if i.startswith("progress:"):
        assert not any(" ago" in x or "updated" in x for x in t), "clock text in " + i
print("   all six cases read right; no clock text in the items")
PY

echo "== 2. both monitors show the same items, and a planted difference is caught"
checked=""
if [ "$(uname)" = Darwin ] && command -v swift >/dev/null && [ -z "${MASON_SKIP_SWIFT:-}" ]; then
  (cd clients/swift && swift build 2>&1 | tail -1)
  start_demo; clients/swift/.build/debug/monitor-tool conformance --data-dir "$root/d" --out "$root/swift" | tail -1; stop_demo
  "$bin/conformance" check --expected "$root/expected" --actual "$root/swift"
  checked="$root/swift"
else
  echo "   (SwiftUI half not run: needs macOS and Swift)"
fi
if [ -z "${MASON_SKIP_COMPOSE:-}" ] && command -v java >/dev/null && command -v gradle >/dev/null; then
  # The core and its tests must build and pass. The app (the windows) is built for the dump; where it
  # cannot be built here, that is said, with the compiler's errors, and the half is not run.
  (cd clients/compose && gradle --no-daemon -q :core:test >"$root/compose-test.log" 2>&1) || { cat "$root/compose-test.log" >&2; echo "the Compose core tests failed" >&2; exit 1; }
  kt=clients/compose/app/build/install/app/bin/app
  rm -f "$kt"
  if ! (cd clients/compose && gradle --no-daemon -q :app:installDist >"$root/compose-build.log" 2>&1); then echo "compose app build failed; see compose-build.log" >&2; fi
  if [ -x "$kt" ]; then
    start_demo; "$kt" conformance --data-dir "$root/d" --out "$root/compose" | tail -1; stop_demo
    "$bin/conformance" check --expected "$root/expected" --actual "$root/compose"
    checked="$root/compose"
  else
    echo "   (Compose half not run: the app did not build here:)"; grep '^e:' "$root/compose-build.log" | head -5 | sed 's/^/     /'
  fi
else
  echo "   (Compose half not run: needs Java and Gradle, and MASON_SKIP_COMPOSE unset)"
fi
if [ -n "$checked" ]; then
  last=$(ls "$checked"/checkpoint-*.json | sort -t- -k2 -n | tail -1)
  mkdir -p "$root/bad"; cp "$checked"/*.json "$root/bad/"
  python3 - "$root/bad/$(basename "$last")" <<'PY'
import json, sys
p = sys.argv[1]
items = json.load(open(p))
for i in items:
    if i["id"] == "progress:5:develop":
        i["texts"][0] = "213 of 340 tests, 62%"
json.dump(items, open(p, "w"))
PY
  if out=$("$bin/conformance" check --expected "$root/expected" --actual "$root/bad" 2>&1); then echo "the planted difference passed" >&2; exit 1; fi
  echo "   a planted difference is caught: $(echo "$out" | head -1)"
fi

echo "== 3. a real daemon: task show --json matches the WebSocket record"
ws=scripts/exit/fixtures/s49/wsclient.py
owner=${USER:-owner}
port=$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1])')
m() { "$bin/mason" --data-dir "$root/d" --config-dir "$root/c" "$@"; }
fail() { set +e; echo "FAILED: $*" >&2; m task list >&2 2>&1; m task show 0 >&2 2>&1; tail -20 "$root/masond.log" >&2; exit 1; }
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
case "$MASON_TITLE" in
  freeze*) i=0; while [ $i -lt 10 ]; do echo "line $i"; i=$((i+1)); sleep 0.2; done; sleep 120 ;;
  *) i=0; while [ $i -lt 60 ]; do echo "tick $i"; i=$((i+1)); sleep 0.3; done; sleep 120 ;;
esac
'''
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
"$bin/masond" --data-dir "$root/d" --config-dir "$root/c" --port "$port" --max-concurrent 4 --heartbeat-ms 100 --sweep-ms 3600000 --owner "$owner" 2>>"$root/masond.log" &
daemon=$!
until m status >/dev/null 2>&1; do kill -0 "$daemon" || { cat "$root/masond.log" >&2; exit 1; }; sleep 0.1; done
add() { m task add "$1" --project web --body "$1" | sed -n 's/^Filed task #\([0-9]*\) .*/\1/p'; }
prog() { m task show "$1" --json | python3 -c "import json,sys; r=json.load(sys.stdin)['progress']['runs']; print(json.dumps(r[0] if r else {}))"; }
lines_of() { prog "$1" | python3 -c "import json,sys; print(json.load(sys.stdin).get('output',{}).get('lines',0))"; }
ge() { [ "$(lines_of "$1")" -ge "$2" ] 2>/dev/null; }

t1=$(add "freeze one")
python3 "$ws" watch "$port" "$t1" 40 "$root/ws.jsonl" &
watcher=$!
wait_for 20 "the frozen step to print its ten lines" ge "$t1" 10
sleep 3   # the step is quiet now; the last push has gone out
cli=$(prog "$t1")
kill "$watcher" 2>/dev/null || true; watcher=""
python3 - "$root/ws.jsonl" "$cli" <<'PY'
import json, sys
recs = [json.loads(l) for l in open(sys.argv[1])]
cli = json.loads(sys.argv[2])
assert recs, "no StepProgress was pushed"
ws = recs[-1]
for k in ("run", "step", "unit", "done", "total", "total_low", "stale_after_ms", "output", "started_ms", "updated_at_ms"):
    assert ws[k] == cli[k], "%s differs: websocket %r, task show --json %r" % (k, ws[k], cli[k])
assert cli["output"]["lines"] == 10 and cli["done"]["source"] == "counted" and cli["unit"] == "lines", cli
assert cli["percent"] is None, "a percentage without a real total: %r" % cli
logs = [l for r in recs for l in r["log"]]
assert logs[:3] == ["line 0", "line 1", "line 2"] and logs[-1] == "line 9", logs
print("   the last StepProgress and `task show --json` agree on %d fields; the pushed log carries all %d lines" % (10, len(logs)))
PY
line=$(m task show "$t1" | grep '^  progress ')
echo "   $line"
echo "$line" | grep -q "10 lines" || fail "task show does not print the count"
echo "$line" | grep -q "updated .* ago" || fail "task show does not print the time since the last update"
echo "$line" | grep -q '%' && fail "a percentage with no real total: $line"

echo "== 4. --watch follows the pushes"
t2=$(add "ticking two")
wait_for 20 "the ticking step to start" ge "$t2" 3
m task show "$t2" --watch >"$root/show-watch.txt" 2>&1 &
w2=$!
m status --watch >"$root/status-watch.txt" 2>&1 &
w3=$!
sleep 6
kill "$w2" "$w3" 2>/dev/null || true; w2=""; w3=""
n=$(awk '/^work  /{c++} END{print c+0}' "$root/show-watch.txt")
[ "$n" -ge 3 ] || fail "task show --watch printed $n progress lines in 6 s: $(cat "$root/show-watch.txt")"
grep -q "^#$t2 work  " "$root/status-watch.txt" || fail "status --watch did not follow task #$t2: $(cat "$root/status-watch.txt")"
echo "   task show --watch printed $n lines in 6 s; status --watch follows $(grep -c '^#' "$root/status-watch.txt") pushes"
echo "s49 passed"
