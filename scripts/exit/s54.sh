#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Self-hosting max_open (s54) exit test: the owner's own tasks are shown, never counted or
# refused (s46, decision 0024), and tasks held behind a blocker are not open.
# A real masond on a project with [self] max_open = 3; every task here is the owner's.
#  1. A chain of five tasks filed with --after (one runnable, four held) is all accepted, and
#     `mason self` shows 0/3 open with 1 of yours open (held ones are not open).
#  2. Two more running tasks, then a fourth past max_open: all accepted, nothing counted,
#     4 of yours open.
#  3. Landing the first releases the second (and only it, the rest wait for their own blockers),
#     one after another in order.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
root=$(mktemp -d /tmp/mason-s54.XXXXXX); daemon=""
cleanup() {
  [ -n "$daemon" ] && kill -9 "$daemon" 2>/dev/null || true
  # Verified before it is killed: a stale pid in a registry may belong to someone else now.
  for hd in "$root"/d/h; do target/debug/step-host-ctl reap "$hd" >/dev/null 2>&1; done
  rm -rf "$root"
}
trap cleanup EXIT
cargo build -q --workspace --bins
bin=target/debug
owner=${USER:-owner}
port=$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1])')
m() { "$bin/mason" --data-dir "$root/d" --config-dir "$root/c" "$@"; }
fail() { set +e; echo "FAILED: $*" >&2; m task list >&2 2>&1; m self >&2 2>&1; tail -20 "$root/masond.log" >&2; exit 1; }
state_of() { m task show "$1" --json 2>/dev/null | sed -n 's/^  "state": "\(.*\)",$/\1/p' | tr 'A-Z' 'a-z'; }
wait_for() {  # SECONDS DESCRIPTION COMMAND...
  local end=$((SECONDS + $1)); local what=$2; shift 2
  until "$@" >/dev/null 2>&1; do [ $SECONDS -lt $end ] || fail "timed out waiting for: $what"; sleep 0.2; done
}
add() {  # TITLE [ARGS...] -> the new task's id
  local t=$1; shift
  m task add "$t" --project web --body "$t" "$@" | sed -n 's/^Filed task #\([0-9]*\) .*/\1/p'
}
self_line() { m self | grep '^web:'; }
landed() { [ "$(state_of "$1")" = landed ]; }
running() { [ "$(state_of "$1")" = developing ]; }
queued() { [ "$(state_of "$1")" = queued ]; }
self_is() { self_line | grep -q "$1"; }

echo "== setup: a bare repository and a pipeline whose work step waits for a gate file named for the task"
mkdir -p "$root/gate" "$root/c/projects"
git init -q --bare -b main "$root/origin.git"
git clone -q "$root/origin.git" "$root/repo" 2>/dev/null
git -C "$root/repo" checkout -q -b main
mkdir -p "$root/repo/.mason"
cat > "$root/repo/.mason/mason.toml" <<TOML
schema = 1
start = "work"

[step.work]
type = "command"
category = "developing"
command = '''
while [ ! -f "$root/gate/\$MASON_TITLE" ]; do sleep 0.2; done
echo "\$MASON_TITLE" > "work-\$MASON_TASK.txt"
git add -A && git -c user.name=t -c user.email=t@t commit -qm "work \$MASON_TASK"
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

[self]
max_open = 3
per_week = 100
TOML
"$bin/masond" --data-dir "$root/d" --config-dir "$root/c" --port "$port" --max-concurrent 4 --heartbeat-ms 100 --sweep-ms 3600000 --owner "$owner" 2>>"$root/masond.log" &
daemon=$!
until m status >/dev/null 2>&1; do kill -0 "$daemon" || { cat "$root/masond.log" >&2; exit 1; }; sleep 0.1; done

echo "== 1. a chain of five with --after: all accepted; 1 of yours open, the four held are not"
c1=$(add "chain 1"); [ -n "$c1" ] || fail "chain 1 was not accepted"
prev=$c1; ids="$c1"
for n in 2 3 4 5; do
  id=$(add "chain $n" --after "$prev"); [ -n "$id" ] || fail "chain $n was refused"
  ids="$ids $id"; prev=$id
done
set -- $ids; c2=$2; c3=$3; c4=$4; c5=$5
wait_for 30 "chain 1 to run" running "$c1"
wait_for 30 "the line to read 0/3 open, 1 of yours" self_is "0/3 open (1 of yours open, not limited)"
echo "   $(self_line)"

echo "== 2. the owner's filings past max_open are accepted and never counted"
r1=$(add "run 1"); r2=$(add "run 2")
[ -n "$r1" ] && [ -n "$r2" ] || fail "the owner's running tasks were refused"
wait_for 30 "the line to read 3 of yours open" self_is "0/3 open (3 of yours open, not limited)"
r3=$(add "past max_open") || true
[ -n "$r3" ] || fail "the owner's filing past max_open was refused"
wait_for 30 "the fourth to run" running "$r3"
wait_for 30 "the line to read 4 of yours open" self_is "0/3 open (4 of yours open, not limited)"
echo "   $(self_line)"

echo "== 3. landing the first releases the held ones, in order"
touch "$root/gate/chain 1"
wait_for 60 "chain 1 to land" landed "$c1"
wait_for 30 "chain 2 to run" running "$c2"
for t in $c3 $c4 $c5; do queued "$t" || fail "task $t left the queue before its blocker landed"; done
echo "   chain 1 landed, chain 2 is running, 3 to 5 still wait: $(self_line)"
for pair in "$c2 $c3 2" "$c3 $c4 3" "$c4 $c5 4"; do
  set -- $pair
  touch "$root/gate/chain $3"
  wait_for 60 "task $1 to land" landed "$1"
  wait_for 30 "task $2 to run" running "$2"
  echo "   chain $3 landed, $2 runs next"
done
touch "$root/gate/chain 5"
wait_for 60 "chain 5 to land" landed "$c5"
touch "$root/gate/run 1" "$root/gate/run 2" "$root/gate/past max_open"
echo "s54 OK"
