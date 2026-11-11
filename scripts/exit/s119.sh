#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s119 exit test: a brief git failure decides nothing. A real masond with the scripted agent;
# the remote is made unreachable (its directory moved away) before the task is filed, so every
# fetch fails. The task's steps start and work from the last-known base (no step fails on the
# fetch), it reaches landing and is held there; when the remote is back it lands.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
root=$(mktemp -d /tmp/mason-s119.XXXXXX); daemon=""
cleanup() {
  [ -n "$daemon" ] && kill -9 "$daemon" 2>/dev/null || true
  for hd in "$root"/d/h; do target/debug/step-host-ctl reap "$hd" >/dev/null 2>&1; done
  rm -rf "$root"
}
trap cleanup EXIT

cargo build -q --workspace --bins
bin=target/debug
m() { "$bin/mason" --data-dir "$root/d" --config-dir "$root/c" "$@"; }
state_of() { m task show "$1" --json 2>/dev/null | sed -n 's/^  "state": "\(.*\)",$/\1/p'; }
wait_state() {  # wait_state TASK STATE SECONDS
  local end=$((SECONDS + $3))
  until [ "$(state_of "$1")" = "$2" ]; do
    [ $SECONDS -lt $end ] || { echo "timed out waiting for task $1 to be $2 (is $(state_of "$1"))" >&2; m task show "$1" >&2; tail -30 "$root/masond.log" >&2; exit 1; }
    sleep 0.2
  done
}

mkdir -p "$root/d" "$root/c/projects"
git init -q --bare -b main "$root/origin.git"
git clone -q "$root/origin.git" "$root/repo"
git -C "$root/repo" checkout -q -b main
echo scratch > "$root/repo/README"
git -C "$root/repo" add -A
git -C "$root/repo" -c user.name=t -c user.email=t@t commit -qm init
git -C "$root/repo" push -q origin main
git -C "$root/repo" fetch -q origin
git -C "$root/repo" checkout -q -b owner-work
cat > "$root/script.json" <<JSON
{"steps": {
  "explore": [[{"call": {"tool": "submit_plan", "args": {"summary": "add hello.txt", "files": ["hello.txt"], "approach": "write the file", "acceptance_criteria": ["hello.txt exists"]}}}]],
  "develop": [[
    {"sh": "echo hello > hello.txt && git add -A && git -c user.name=t -c user.email=t@t commit -qm 'add hello'"},
    {"call": {"tool": "submit_change", "args": {"summary": "added hello.txt", "head": "\$HEAD"}}}
  ]]
}}
JSON
cat > "$root/c/projects/scratch.toml" <<TOML
path = "$root/repo"
base = "main"
remote = "origin"
test = "test -f hello.txt"
claude_bin = "$PWD/$bin/fake-claude"
[agent_env]
FAKE_CLAUDE_SCRIPT = "$root/script.json"
TOML

"$bin/masond" --port 0 --data-dir "$root/d" --config-dir "$root/c" --heartbeat-ms 100 --sweep-ms 3600000 2>>"$root/masond.log" &
daemon=$!
until m status >/dev/null 2>&1; do kill -0 "$daemon" || { echo "masond died" >&2; cat "$root/masond.log" >&2; exit 1; }; sleep 0.1; done

# The remote goes away: every fetch and push now fails.
mv "$root/origin.git" "$root/origin.gone"
echo "the remote is unreachable"

t=$(m task add "Add a hello file" --body "Create hello.txt containing hello." | sed -n 's/^Filed task #\([0-9]*\) .*/\1/p')
[ -n "$t" ] || { echo "the task was not filed" >&2; exit 1; }
wait_state "$t" landing 120
echo "the task's steps ran from the last-known base and it waits to land"
sleep 3
[ "$(state_of "$t")" = landing ] || { echo "the task left landing while the remote was away: $(state_of "$t")" >&2; m task show "$t" >&2; tail -30 "$root/masond.log" >&2; exit 1; }

mv "$root/origin.gone" "$root/origin.git"
echo "the remote is back"
wait_state "$t" landed 120
[ "$(git -C "$root/origin.git" show main:hello.txt)" = hello ]
echo "s119 OK"
