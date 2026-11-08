#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Stage 6 exit test: land a task on a scratch repository with the fake agent,
# sending kill -9 to the daemon in the middle of the agent's session, and
# confirm nothing is lost. Then run the whole crash-injection harness.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
root=$(mktemp -d /tmp/mason-s06.XXXXXX); daemon=""
cleanup() {
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
m() { "$bin/mason" --data-dir "$root/d" --config-dir "$root/c" "$@"; }
start_daemon() {
  "$bin/masond" --port 0 --data-dir "$root/d" --config-dir "$root/c" --heartbeat-ms 100 --sweep-ms 3600000 2>>"$root/masond.log" &
  daemon=$!
  until m status >/dev/null 2>&1; do kill -0 "$daemon" || { echo "masond died" >&2; cat "$root/masond.log" >&2; exit 1; }; sleep 0.1; done
}
wait_state() {  # wait_state TASK STATE SECONDS
  local end=$((SECONDS + $3))
  until [ "$(m task show "$1" --json 2>/dev/null | sed -n 's/^  "state": "\(.*\)",$/\1/p')" = "$2" ]; do
    [ $SECONDS -lt $end ] || { echo "timed out waiting for task $1 to be $2" >&2; m task show "$1" >&2 || true; exit 1; }
    sleep 0.2
  done
}

# A scratch repository with a bare "origin", and a scripted stand-in for the agent.
git init -q --bare -b main "$root/origin.git"
git clone -q "$root/origin.git" "$root/repo"
git -C "$root/repo" checkout -q -b main
echo scratch > "$root/repo/README"
git -C "$root/repo" add -A
git -C "$root/repo" -c user.name=t -c user.email=t@t commit -qm init
git -C "$root/repo" push -q origin main
git -C "$root/repo" checkout -q -b owner-work
mkdir -p "$root/c/projects"
cat > "$root/script.json" <<JSON
{"steps": {
  "explore": [[{"call": {"tool": "submit_plan", "args": {"summary": "add hello.txt", "files": ["hello.txt"], "approach": "write the file", "acceptance_criteria": ["hello.txt exists"]}}}]],
  "develop": [[
    {"sh": "echo hello > hello.txt && git add -A && git -c user.name=t -c user.email=t@t commit -qm 'add hello'"},
    {"sleep_ms": 5000},
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

m doctor | grep -v '^warn'
start_daemon
m task add "Add a hello file" --body "Create hello.txt containing hello." > /dev/null
wait_state 0 developing 60
until git -C "$root/repo" rev-list --count origin/main..mason/scratch/t0 2>/dev/null | grep -qx 1; do sleep 0.1; done
echo "agent is mid-session: its branch has the commit, the submission is 5 s away"

kill -9 "$daemon"; wait "$daemon" 2>/dev/null || true; daemon=""
host_pid=$(sed -n 's/^host_pid=//p' "$root"/d/h/t0r1-1/host.kv)
kill -0 "$host_pid" || { echo "the agent's step host died with the daemon" >&2; exit 1; }
echo "daemon killed with kill -9; the agent's step host (pid $host_pid) is still running"
sleep 1

start_daemon
echo "daemon restarted; waiting for the task to finish"
wait_state 0 landed 120

n=$(git -C "$root/origin.git" log --format=%s main | grep -cx 'add hello' || true)
[ "$n" = 1 ] || { echo "the commit is on the remote $n times" >&2; exit 1; }
[ "$(git -C "$root/origin.git" show main:hello.txt)" = hello ]
devs=$(m task show 0 --json | grep -c '"step": "developing"' || true)
[ "$devs" = 1 ] || { echo "the develop step was run $devs times" >&2; exit 1; }
[ "$(m status --json | sed -n 's/^  "bugs": \(.*\),$/\1/p')" = 0 ]
echo "landed exactly once; one develop step; the same agent session carried on; no bugs filed"
m task show 0 | sed -n '1,8p;/^steps:/,$p'

# Everything else: the crash-injection harness, the reconcile sweep, the end-to-end
# tests, replay of recorded runs, and the git and landing tests.
trap - EXIT; cleanup
# (CI sets MASON_TESTS_ALREADY_RAN after `cargo test --workspace`.)
[ -n "${MASON_TESTS_ALREADY_RAN:-}" ] || cargo test -p mason-landing -p mason-daemon -p mason-pipeline
echo "s06 OK"
