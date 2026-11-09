#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s92 exit test: a real daemon over a generated database with thousands of events starts,
# answers `mason status`, and a new task runs and lands. Replay time is not asserted
# (timing is load-dependent); the clone counts are in crates/store/tests/no_copy.rs.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
root=$(mktemp -d /tmp/mason-s92.XXXXXX); daemon=""
cleanup() {
  [ -n "$daemon" ] && kill -9 "$daemon" 2>/dev/null || true
  target/debug/step-host-ctl reap "$root/d/h" >/dev/null 2>&1 || true
  rm -rf "$root"
}
trap cleanup EXIT

cargo build -q --workspace --bins
cargo build -q -p mason-store --example gen_log
bin=target/debug
m() { "$bin/mason" --data-dir "$root/d" --config-dir "$root/c" "$@"; }
wait_state() {  # wait_state TASK STATE SECONDS
  local end=$((SECONDS + $3))
  until [ "$(m task show "$1" --json 2>/dev/null | sed -n 's/^  "state": "\(.*\)",$/\1/p')" = "$2" ]; do
    [ $SECONDS -lt $end ] || { echo "timed out waiting for task $1 to be $2" >&2; m task show "$1" >&2 || true; exit 1; }
    sleep 0.2
  done
}

tasks=300
mkdir -p "$root/d" "$root/c/projects"
"$bin/examples/gen_log" gen "$root/d/store.db" "$tasks"
echo "generated a database of $tasks landed tasks"

git init -q --bare -b main "$root/origin.git"
git clone -q "$root/origin.git" "$root/repo"
git -C "$root/repo" checkout -q -b main
echo scratch > "$root/repo/README"
git -C "$root/repo" add -A
git -C "$root/repo" -c user.name=t -c user.email=t@t commit -qm init
git -C "$root/repo" push -q origin main
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
echo "the daemon is up over the generated log and answers status"

m task add "Add a hello file" --body "Create hello.txt containing hello." > /dev/null
wait_state "$tasks" landed 120
[ "$(git -C "$root/origin.git" show main:hello.txt)" = hello ]
[ "$(m status --json | sed -n 's/^  "bugs": \(.*\),$/\1/p')" = 0 ]
echo "s92 OK"
