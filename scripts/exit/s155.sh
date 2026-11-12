#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Outside pipelines queue in masond (s155) exit test: a real masond with `[llm] claude = 1` and the
# fake agent.
#  1. `mason hold --needs llm:claude --pipeline other` prints `granted` once and holds the one slot.
#  2. A MASON task's agent step waits for it; `mason resources` lists the holder with its pipeline.
#  3. Closing the hold's stdin releases it, the agent starts and the task lands.
#  4. SIGTERM, SIGKILL and the exit of --pid each free the slot.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
root=$(mktemp -d /tmp/mason-s155.XXXXXX); daemon=""; holds=""
cleanup() {
  exec 3>&- 2>/dev/null || true
  for p in $holds; do kill -9 "$p" 2>/dev/null || true; done
  [ -n "$daemon" ] && kill -9 "$daemon" 2>/dev/null || true
  for hd in "$root"/d/h; do target/debug/step-host-ctl reap "$hd" >/dev/null 2>&1; done
  rm -rf "$root"
}
trap cleanup EXIT

cargo build -q --workspace --bins
bin=target/debug
m() { "$bin/mason" --data-dir "$root/d" --config-dir "$root/c" "$@"; }
fail() { set +e; echo "FAILED: $*" >&2; m resources >&2 2>&1; tail -20 "$root/masond.log" >&2; exit 1; }
jq_state() { sed -n 's/^  "state": "\(.*\)",$/\1/p'; }
task_id() { sed -n 's/.*#\([0-9]*\).*/\1/p'; }
wait_for() {  # SECONDS DESCRIPTION COMMAND...
  local end=$((SECONDS + $1)) what=$2; shift 2
  until "$@"; do [ $SECONDS -lt $end ] || fail "timed out waiting for $what"; sleep 0.2; done
}
used0() { m resources | grep -q 'llm:claude .* 0/1 in use'; }
used1() { m resources | grep -q 'llm:claude .* 1/1 in use'; }
state_is() { [ "$(m task show "$1" --json 2>/dev/null | jq_state)" = "$2" ]; }
gone() { ! kill -0 "$1" 2>/dev/null; }
granted() { [ "$(grep -c '^granted$' "$1" 2>/dev/null)" = 1 ]; }
pusher() {  # BRANCH PATH MESSAGE
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
mkdir -p "$root/c/projects" "$root/d"
printf '[llm]\nclaude = 1\n' > "$root/c/resources.toml"
turn='{"sh": "echo hello > hello.txt && git add -A && git -c user.name=t -c user.email=t@t commit -qm work"}, {"call": {"tool": "submit_change", "args": {"summary": "did it", "head": "$HEAD"}}}'
echo "{\"steps\": {\"develop\": [[$turn], [$turn]]}}" > "$root/work.json"
cat > "$root/c/projects/worker.toml" <<TOML
path = "$root/repo"
base = "main"
remote = "origin"
test = "false"
claude_bin = "$PWD/$bin/fake-claude"
[agent_env]
FAKE_CLAUDE_SCRIPT = "$root/work.json"
TOML
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
suites = ["quick"]
on_red = "develop"
max_rounds = 3
next = "land"

[step.land]
type = "landing"
category = "landing"

[suite.quick]
command = "sh quick.sh"
parser = "tap"
min_tests = 1
level = "unit"
TOML
printf '#!/bin/sh\necho 1..1; echo "ok 1 - quick"\n' | pusher main quick.sh "a suite"

"$bin/masond" --port 0 --data-dir "$root/d" --config-dir "$root/c" --heartbeat-ms 100 --sweep-ms 3600000 --max-concurrent 3 --owner "${USER:-owner}" 2>>"$root/masond.log" &
daemon=$!
until m status >/dev/null 2>&1; do kill -0 "$daemon" || { cat "$root/masond.log" >&2; exit 1; }; sleep 0.1; done
used0 || fail "the llm limit is not 1"

# A hold whose stdin is a fifo we keep open on fd 3; closing fd 3 is the end of stdin.
start_hold() {  # NAME [extra mason hold flags...] -> sets $hold_pid, fd 3 open
  local name=$1; shift
  rm -f "$root/in-$name"; mkfifo "$root/in-$name"
  m hold --needs llm:claude --pipeline other --label "$name" "$@" < "$root/in-$name" > "$root/out-$name" 2>"$root/err-$name" &
  hold_pid=$!; holds="$holds $hold_pid"
  exec 3> "$root/in-$name"
  wait_for 30 "hold $name to be granted" granted "$root/out-$name"
}

echo "== 1. a hold is granted once and holds the slot"
start_hold first
used1 || fail "the hold does not count against llm:claude"
[ "$(cat "$root/out-first")" = granted ] || fail "the hold printed more than one line"
echo "   granted"

echo "== 2. a MASON task's agent step waits for it, and the holder is listed with its pipeline"
t=$(m task add "Needs a session" --project worker --explored --body "x" | task_id)
# Admission counts the hold against llm:claude, so the task is not even claimed: no session is free.
sleep 3
st=$(m task show "$t" --json | jq_state)
case "$st" in landed|developing|testing) fail "the task is $st while the hold held the only slot";; esac
echo "   task is $st"
m resources | grep 'other first' | sed 's/^/   /'
m resources | grep -q 'other first: holding llm:claude' || fail "the holder is not listed with its pipeline"
m resources --json | grep -q '"pipeline": "other"' || fail "the JSON has no pipeline"
m status | grep -q 'other pipeline: other first: holding llm:claude' || fail "mason status does not show it"
echo "   waiting"

echo "== 3. stdin closes: the hold ends, the agent starts and the task lands"
exec 3>&-
wait_for 30 "the hold to exit" gone "$hold_pid"
wait_for 120 "the task to land" state_is "$t" landed
echo "   landed"

echo "== 4. SIGTERM, SIGKILL and the exit of --pid each free the slot"
wait_for 30 "the slot to be free" used0
start_hold term
kill -TERM "$hold_pid"; wait_for 30 "SIGTERM to end the hold" gone "$hold_pid"
exec 3>&-
wait_for 30 "the slot to be free after SIGTERM" used0
echo "   SIGTERM freed it"
start_hold kill
kill -KILL "$hold_pid"; wait_for 30 "SIGKILL to end the hold" gone "$hold_pid"
exec 3>&-
wait_for 30 "the slot to be free after SIGKILL" used0
echo "   SIGKILL freed it"
sleep 600 & target=$!; holds="$holds $target"
start_hold pid --pid "$target"
used1 || fail "the --pid hold is not counted"
kill "$target"; wait_for 30 "the hold to notice --pid exiting" gone "$hold_pid"
exec 3>&-
wait_for 30 "the slot to be free after --pid exit" used0
echo "   --pid exit freed it"
