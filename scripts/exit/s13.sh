#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Stage 13 exit test: the Compose monitor beside the SwiftUI one.
#  1. The Compose project builds and its tests pass.
#  2. Both clients run the same conformance scenarios against `masond --demo` and
#     match what the daemon's views require, and each other.
#  3. The feature check passes: every feature is done in both clients.
#  4. Against a real daemon, answering from either monitor resumes a task, and
#     cancel and skip work from both.
# The SwiftUI half needs macOS and Swift; it is skipped where they are missing.
# Skipped, too, when MASON_SKIP_COMPOSE is set (CI's Rust job: the clients have jobs of their own).
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
if [ -n "${MASON_SKIP_COMPOSE:-}" ] || ! command -v java >/dev/null || ! command -v gradle >/dev/null; then
  echo "s13 skipped: needs Java and Gradle"; exit 0
fi
swift_ok=0
if [ "$(uname)" = Darwin ] && command -v swift >/dev/null && [ -z "${MASON_SKIP_SWIFT:-}" ]; then swift_ok=1; fi
root=$(mktemp -d /tmp/mason-s13.XXXXXX); daemon=""
cleanup() {
  [ -n "$daemon" ] && kill -9 "$daemon" 2>/dev/null || true
  # Verified before it is killed: a stale pid in a registry may belong to someone else now.
  for hd in "$root"/d/h; do target/debug/step-host-ctl reap "$hd" >/dev/null 2>&1; done
  rm -rf "$root"
}
trap cleanup EXIT

cargo build -q --workspace --bins
bin=target/debug
echo "== 1. the Compose project builds and its tests pass"
(cd clients/compose && gradle --no-daemon -q :core:test :app:installDist 2>&1 | grep -vE '^$' | head -5 || true)
kt=clients/compose/app/build/install/app/bin/app
test -x "$kt"
python3 - <<'PY'
import glob, re
t = f = 0
for p in glob.glob("clients/compose/core/build/test-results/test/*.xml"):
    m = re.search(r'tests="(\d+)" skipped="\d+" failures="(\d+)" errors="(\d+)"', open(p).read())
    t += int(m.group(1)); f += int(m.group(2)) + int(m.group(3))
print(f"{t} Compose tests, {f} failures")
assert t > 0 and f == 0
PY
if [ $swift_ok = 1 ]; then
  (cd clients/swift && swift build 2>&1 | tail -1)
  sw=clients/swift/.build/debug/monitor-tool
fi

start_demo() {
  rm -rf "$root/d"; mkdir -p "$root/d"
  "$bin/masond" --port 0 --demo-manual --data-dir "$root/d" --config-dir "$root/c" 2>>"$root/masond.log" &
  daemon=$!
  until [ -s "$root/d/daemon.json" ]; do kill -0 "$daemon" || { cat "$root/masond.log" >&2; exit 1; }; sleep 0.1; done
}
stop_demo() { kill -9 "$daemon" 2>/dev/null || true; wait "$daemon" 2>/dev/null || true; daemon=""; }
port() { sed -n 's/.*"port": *\([0-9]*\).*/\1/p' "$root/d/daemon.json"; }

echo "== 2. the same conformance scenarios, both clients"
start_demo; "$bin/conformance" expected --port "$(port)" --out "$root/expected" | tail -1; stop_demo
start_demo; "$kt" conformance --data-dir "$root/d" --out "$root/compose" | tail -1; stop_demo
"$bin/conformance" check --expected "$root/expected" --actual "$root/compose"
if [ $swift_ok = 1 ]; then
  start_demo; "$sw" conformance --data-dir "$root/d" --out "$root/swift" | tail -1; stop_demo
  "$bin/conformance" check --expected "$root/expected" --actual "$root/swift"
  # And the two against each other: whatever one shows, the other shows.
  "$bin/conformance" check --expected "$root/swift" --actual "$root/compose"
  echo "(SwiftUI and Compose show the same thing at every checkpoint)"
fi
# A planted difference in the Compose dump is caught.
mkdir -p "$root/bad"; cp "$root/compose"/*.json "$root/bad/"
python3 - "$root/bad/checkpoint-7.json" <<'PY'
import json, sys
p = sys.argv[1]
items = json.load(open(p))
for i in items:
    if i["id"] == "row:testing:1":
        i["actions"] = i["actions"] + ["delete"]
json.dump(items, open(p, "w"))
PY
if out=$("$bin/conformance" check --expected "$root/expected" --actual "$root/bad" 2>&1); then echo "the planted difference passed" >&2; exit 1; fi
echo "$out" | head -2

if [ -z "${MASON_NO_GUI:-}" ]; then
  echo "== 2b. the real Compose window launches and connects to the daemon"
  start_demo
  java -cp "clients/compose/app/build/install/app/lib/*" mason.monitor.MainKt --data-dir "$root/d" >"$root/app.log" 2>&1 &
  app=$!
  n=0
  for _ in $(seq 1 60); do
    n=$(lsof -nP -iTCP -sTCP:ESTABLISHED -a -p "$app" 2>/dev/null | grep -c ESTABLISHED || true)
    [ "$n" -gt 0 ] && break; sleep 0.5
  done
  kill -9 "$app" 2>/dev/null || true
  [ "$n" -gt 0 ] || { cat "$root/app.log" >&2; echo "the Compose window did not connect" >&2; exit 1; }
  echo "the window is running and holds a connection to masond"
  stop_demo
fi

echo "== 3. every feature is done in both clients"
"$bin/conformance" features clients/features.toml
"$bin/conformance" tokens --check

echo "== 4. against a real daemon: answer, cancel and skip from either monitor"
m() { "$bin/mason" --data-dir "$root/d" --config-dir "$root/c" "$@"; }
jq_state() { sed -n 's/^  "state": "\(.*\)",$/\1/p'; }
wait_state() {  # TASK STATE SECONDS
  local end=$((SECONDS + $3))
  until [ "$(m task show "$1" --json 2>/dev/null | jq_state)" = "$2" ]; do
    [ $SECONDS -lt $end ] || { echo "timed out waiting for task $1 to be $2" >&2; m task show "$1" >&2 || true; exit 1; }
    sleep 0.2
  done
}
wait_slow() {  # TASK: until its slow suite is running
  local end=$((SECONDS + 60))
  until ls "$root"/d/h 2>/dev/null | grep -q "^t${1}r[0-9]*-u-slow$"; do
    [ $SECONDS -lt $end ] || { echo "the slow suite never started for task $1" >&2; m task show "$1" >&2; exit 1; }
    sleep 0.2
  done
}
task_id() { sed -n 's/.*#\([0-9]*\).*/\1/p'; }
pusher() {
  [ -d "$root/pusher" ] || git clone -q "$root/origin.git" "$root/pusher" 2>/dev/null
  git -C "$root/pusher" fetch -q origin
  git -C "$root/pusher" checkout -q -B "$1" "origin/$1"
  mkdir -p "$root/pusher/$(dirname "$2")"; cat > "$root/pusher/$2"
  git -C "$root/pusher" add -A
  git -C "$root/pusher" -c user.name=t -c user.email=t@t commit -qm "$3"
  git -C "$root/pusher" push -q origin "HEAD:$1"
}
git init -q --bare -b main "$root/origin.git"
git clone -q "$root/origin.git" "$root/repo" 2>/dev/null
git -C "$root/repo" checkout -q -b main
echo scratch > "$root/repo/README"
git -C "$root/repo" add -A
git -C "$root/repo" -c user.name=t -c user.email=t@t commit -qm init
git -C "$root/repo" push -q origin main
git -C "$root/repo" checkout -q -b owner-work
mkdir -p "$root/c/projects"
turn='{"sh": "echo hello > hello.txt && echo $PWD > unique-$(basename \"$PWD\").txt && git add -A && git -c user.name=t -c user.email=t@t commit -qm work"}, {"call": {"tool": "submit_change", "args": {"summary": "did it", "head": "$HEAD"}}}'
cat > "$root/ask.json" <<JSON
{"steps": {"develop": [
  [{"call": {"tool": "ask", "args": {
    "ask": "Should the token expire after one hour or one day?",
    "why": "The ticket does not say and the security notes are silent on it.",
    "options": [{"label": "One hour, with refresh tokens", "recommended": true}, {"label": "One day, without refresh tokens"}]}}}],
  [$turn], [$turn]]}}
JSON
cat > "$root/work.json" <<JSON
{"steps": {"develop": [[$turn], [$turn]]}}
JSON
for p in asker:ask worker:work; do
  cat > "$root/c/projects/${p%%:*}.toml" <<TOML
path = "$root/repo"
base = "main"
remote = "origin"
test = "true"
claude_bin = "$PWD/$bin/fake-claude"
[agent_env]
FAKE_CLAUDE_SCRIPT = "$root/${p##*:}.json"
TOML
done
pusher main .mason/mason.toml "definition" <<'TOML'
schema = 1
start = "develop"

[step.develop]
type = "agent"
category = "developing"
next = "test"

[step.test]
type = "test"
category = "testing"
suites = ["slow"]
on_red = "develop"
max_rounds = 3
next = "land"

[step.land]
type = "landing"
category = "landing"

[suite.slow]
command = "sh slow.sh"
parser = "tap"
min_tests = 1
level = "unit"
TOML
printf '#!/bin/sh\nif [ -f %s/slow.flag ]; then sleep 120; fi\necho 1..1; echo "ok 1 - slow"\n' "$root" | pusher main slow.sh "a suite"
rm -rf "$root/d"; mkdir -p "$root/d"
"$bin/masond" --port 0 --data-dir "$root/d" --config-dir "$root/c" --heartbeat-ms 100 --sweep-ms 3600000 --max-concurrent 8 2>>"$root/masond.log" &
daemon=$!
until m status >/dev/null 2>&1; do kill -0 "$daemon" || { cat "$root/masond.log" >&2; exit 1; }; sleep 0.1; done
# At start the daemon reads only what is already fetched, and the definition above was pushed
# after the clone: until the watcher's first fetch the built-in pipeline is in force, and a task
# admitted then runs `test = "true"` instead of the slow suite. Fetch and apply it now.
for p in asker worker; do
  m definition reload --project "$p" | grep -qE 'Applied|Nothing changed' \
    || { echo "the $p definition did not apply" >&2; m definition show --project "$p" >&2; exit 1; }
done
dport=$(port)
touch "$root/slow.flag"

# Answers: one task answered from each monitor.
a=$(m task add "Answered from SwiftUI" --project asker --explored --body x | task_id)
b=$(m task add "Answered from Compose" --project asker --explored --body x | task_id)
# Cancels and skips: tasks whose slow suite is running.
c1=$(m task add "Cancelled from SwiftUI" --project worker --explored --body x | task_id)
c2=$(m task add "Cancelled from Compose" --project worker --explored --body x | task_id)
s1=$(m task add "Skipped from SwiftUI" --project worker --explored --body x | task_id)
s2=$(m task add "Skipped from Compose" --project worker --explored --body x | task_id)
for t in $a $b; do wait_state "$t" needs_attention 60; done
for t in $c1 $c2 $s1 $s2; do wait_slow "$t"; done
echo "tasks $a $b are waiting for the owner; $c1 $c2 $s1 $s2 are running their slow suite"

if [ $swift_ok = 1 ]; then
  "$sw" answer --port "$dport" --task "$a" --option 1 | tail -1
  "$sw" cancel --port "$dport" --task "$c1"
  "$sw" skip --port "$dport" --task "$s1" --step test
fi
"$kt" answer --port "$dport" --task "$b" --option 1 | tail -1
"$kt" cancel --port "$dport" --task "$c2"
"$kt" skip --port "$dport" --task "$s2" --step test
rm -f "$root/slow.flag"

for t in $b $s2; do wait_state "$t" landed 120; done
wait_state "$c2" canceled 30
if [ $swift_ok = 1 ]; then
  for t in $a $s1; do wait_state "$t" landed 120; done
  wait_state "$c1" canceled 30
fi
echo "answered tasks resumed and landed; cancelled tasks are cancelled; skipped tasks landed:"
m task show "$s2" | grep -E 'tests |debt'
m task show "$s2" | grep -q 'tests     unverified: .*slow: skipped by force'
m debt list | grep -q "task #$s2 .*step:test"
m metrics
n=$(m metrics --json | tr -d ' \n')
want=1; [ $swift_ok = 1 ] && want=2
for kind in answer cancel skip_test; do
  echo "$n" | grep -q "\"$kind\":$want" || { echo "expected $want $kind intervention(s)" >&2; m metrics >&2; exit 1; }
done
echo "(every action from either monitor left its record)"

trap - EXIT; cleanup
echo "s13 OK"
