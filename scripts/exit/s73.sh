#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Stage 73 exit test: the monitor's left panel as the daemon sends it.
#  0. cargo: the reducer's state_entered_at tests and the protocol sidebar test.
#  1. A real masond (scripted agent fake-claude; no model, no network) with a test step that
#     hangs, so tasks stay where they get to. Tasks of different priorities are filed, some
#     move into Testing, some are held by a blocker, some are canceled.
#  2. The sidebar is read from the daemon's view over the websocket (not the CLI):
#       Queued: urgent, then normal, then low, though filed low, normal, urgent;
#       Testing: the task that entered it last on top;
#       Canceled: by when they were canceled, not when they were filed.
#  3. After a masond restart the order is the same.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
echo "== 0. cargo tests"
cargo test -q -p mason-engine --test state_entered
cargo test -q -p mason-daemon --test protocol the_sidebar_is_in_queue_order_and_the_rest_newest_entry_first
root=$(mktemp -d /tmp/mason-s73.XXXXXX); daemon=""
stop_daemon() {
  if [ -n "$daemon" ]; then kill -9 "$daemon" 2>/dev/null || true; wait "$daemon" 2>/dev/null || true; fi
  daemon=""
}
kill_hosts() { for hd in "$root"/d/h; do target/debug/step-host-ctl reap "$hd" >/dev/null 2>&1 || true; done; }
cleanup() { stop_daemon; kill_hosts; rm -rf "$root"; }
trap cleanup EXIT
cargo build -q --workspace --bins
bin=target/debug
owner=${USER:-owner}
cdir="$root/c"; mkdir -p "$cdir/projects"
m() { "$bin/mason" --data-dir "$root/d" --config-dir "$cdir" "$@"; }
pyget() { python3 -I -c 'import json,sys; v=json.load(sys.stdin); print(v.get(sys.argv[1], ""))' "$1"; }
die() { echo "$1" >&2; tail -20 "$root/masond.log" >&2; exit 1; }
wait_state() {  # TASK STATE SECONDS
  local end=$((SECONDS + $3))
  until [ "$(m task show "$1" --json 2>/dev/null | pyget state)" = "$2" ]; do
    [ $SECONDS -lt $end ] || { m task show "$1" >&2; die "timed out waiting for task $1 to be $2"; }
    sleep 0.3
  done
}
sidebar() { python3 -I scripts/exit/fixtures/s73/sidebar.py "$root/d"; }
line() { sidebar | sed -n "s/^$1: //p"; }  # the rows of one section
expect() {  # SECTION WANT
  local got; got=$(line "$1")
  [ "$got" = "$2" ] || { sidebar >&2; die "$1: wanted [$2], the daemon sent [$got]"; }
  echo "   $1: $got"
}
repo="$root/r"; origin="$root/o.git"
git init -q --bare -b main "$origin" 2>/dev/null
git clone -q "$origin" "$repo" 2>/dev/null
git -C "$repo" checkout -q -b main
mkdir -p "$repo/.mason"
cat > "$repo/.mason/mason.toml" <<'TOML'
schema = 1
start = "explore"

[step.explore]
type = "agent"
category = "exploring"
next = "develop"

[step.develop]
type = "agent"
category = "developing"
next = "test"

[step.test]
type = "test"
category = "testing"
max_rounds = 2
on_red = "develop"
next = "land"

[step.land]
type = "landing"
category = "landing"
TOML
printf 'mode = "direct"\n' > "$repo/.mason/landing.toml"
echo base > "$repo/README"
git -C "$repo" add -A
git -C "$repo" -c user.name=t -c user.email=t@t commit -qm pipeline
git -C "$repo" push -q origin main
git -C "$repo" checkout -q -b owner-work

cat > "$root/script.json" <<'JSON'
{"steps": {
  "explore": [[{"call": {"tool": "submit_plan", "args": {"summary": "add a file", "files": ["f.txt"], "approach": "write it", "acceptance_criteria": ["f.txt exists"]}}}]],
  "develop": [[
    {"sh": "echo hello > f-$(date +%s%N).txt && git add -A && git -c user.name=t -c user.email=t@t commit -qm hello"},
    {"call": {"tool": "submit_change", "args": {"summary": "added a file", "head": "$HEAD"}}}
  ]]
}}
JSON
# The test command hangs, so a task that reaches Testing stays there.
printf 'sleep 600\n' > "$root/t.sh"
cat > "$cdir/projects/p.toml" <<TOML
path = "$repo"
base = "main"
remote = "origin"
test = "sh $root/t.sh"
claude_bin = "$PWD/$bin/fake-claude"
[agent_env]
FAKE_CLAUDE_SCRIPT = "$root/script.json"
TOML
: > "$root/masond.log"
"$bin/masond" --port 0 --data-dir "$root/d" --config-dir "$cdir" --heartbeat-ms 100 --sweep-ms 3600000 --owner "$owner" 2>>"$root/masond.log" &
daemon=$!
until m status >/dev/null 2>&1; do kill -0 "$daemon" || { cat "$root/masond.log" >&2; exit 1; }; sleep 0.1; done

start_daemon() {
  "$bin/masond" --port 0 --data-dir "$root/d" --config-dir "$cdir" --heartbeat-ms 100 --sweep-ms 3600000 --owner "$owner" 2>>"$root/masond.log" &
  daemon=$!
  until m status >/dev/null 2>&1; do kill -0 "$daemon" || { cat "$root/masond.log" >&2; exit 1; }; sleep 0.1; done
}
: > "$root/masond.log"
start_daemon

echo "== 1. file tasks"
# #0 and #1 go to Testing, #1 first; #0 is filed first, so filing order and entry order differ.
m task add "zero" --project p --body x >/dev/null
wait_state 0 testing 120
m task add "one" --project p --body x >/dev/null
wait_state 1 testing 120
# Held by #0 (which never leaves Testing): they stay queued. Filed low, normal, urgent.
m task add "low" --project p --body x --priority low --after 0 >/dev/null
m task add "normal" --project p --body x --priority normal --after 0 >/dev/null
m task add "urgent" --project p --body x --priority urgent --after 0 >/dev/null
# Two more, canceled in the opposite order to their filing.
m task add "c5" --project p --body x --after 0 >/dev/null
m task add "c6" --project p --body x --after 0 >/dev/null
m task cancel 6 >/dev/null
m task cancel 5 >/dev/null

check() {
  expect queued "#4 #3 #2"
  expect testing "#1 #0"
  expect canceled "#5 #6"
}
echo "== 2. the daemon's sidebar"
check
echo "== 3. after a restart"
stop_daemon
start_daemon
sleep 1
check
echo "s73 passed"
