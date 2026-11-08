#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Stage 17, manual: MASON lands a real small change to *its own source*, then
# updates itself to the build that contains it while another session is running,
# and that session is not interrupted.
#
# Everything happens in a scratch copy of this repository in /tmp (a bare "origin"
# made from HEAD, and a clone); the real repository and its remote are never
# touched, and MASON is not turned on for them. The agent is the scripted
# stand-in, so no model is called and nothing is spent. Takes several minutes: it
# builds this repository in release mode once.
#
#   1. a self-change (a line in docs/self-hosting.md) is filed, passes the pipeline
#      and lands on the scratch origin's `main`;
#   2. `mason update --check` builds that `main` and proves it (self-test);
#   3. a second task starts a 40-second agent session;
#   4. `mason update` takes over from the running daemon during that session;
#   5. the session is the same process, and its task lands once.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
root=$(mktemp -d /tmp/mason-s17real.XXXXXX); daemon=""
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
bin=$PWD/target/debug
owner=${USER:-owner}
m() { "$bin/mason" --data-dir "$root/d" --config-dir "$root/c" "$@"; }
jq_state() { sed -n 's/^  "state": "\(.*\)",$/\1/p'; }
wait_state() {  # TASK STATE SECONDS
  local end=$((SECONDS + $3))
  until [ "$(m task show "$1" --json 2>/dev/null | jq_state)" = "$2" ]; do
    [ $SECONDS -lt $end ] || { echo "timed out waiting for task $1 to be $2" >&2; m task show "$1" >&2 || true; exit 1; }
    sleep 1
  done
}
daemon_pid() { sed -n 's/.*"pid": *\([0-9]*\).*/\1/p' "$root/d/daemon.json"; }

echo "== setting up a scratch copy of MASON's source in $root"
git clone -q --bare . "$root/origin.git"
git -C "$root/origin.git" branch -f main HEAD
git clone -q "$root/origin.git" "$root/repo" 2>/dev/null
git -C "$root/repo" checkout -q main
# The repository's own landing config collects a batch for 10 minutes; in this scratch
# copy (only) it goes at once, so that the run does not sit waiting for its window.
sed -i.bak 's/^window .*/window   = "0s"/' "$root/repo/.mason/landing.toml" && rm -f "$root/repo/.mason/landing.toml.bak"
git -C "$root/repo" -c user.name=t -c user.email=t@t commit -qam "scratch: no batch window"
git -C "$root/repo" push -q origin main
git -C "$root/repo" checkout -q -b owner-work
mkdir -p "$root/c/projects"
# Task 0 is the self-change; task 1 is a long session (the directory is named after the task).
turn='{"sh": "case $(basename \"$PWD\") in 0) echo \"- Landed by MASON itself (a scratch run of s17-real.sh).\" >> docs/self-hosting.md;; 1) sleep 40; echo \"session finished\" > docs/session-note.md;; esac; git add -A && git -c user.name=t -c user.email=t@t commit -qm \"change from task $(basename \"$PWD\")\""}, {"call": {"tool": "submit_change", "args": {"summary": "done", "head": "$HEAD"}}}'
echo "{\"steps\": {\"develop\": [[$turn]]}}" > "$root/script.json"
cat > "$root/c/projects/mason.toml" <<TOML
path = "$root/repo"
base = "main"
remote = "origin"
test = "cargo check -q -p mason-core"
claude_bin = "$bin/fake-claude"
step_timeout_s = 600
test_timeout_s = 600
[self]
holds = "owner"
[agent_env]
FAKE_CLAUDE_SCRIPT = "$root/script.json"
TOML
printf 'repo = "%s"\nchannel = "main"\n' "$root/origin.git" > "$root/c/update.toml"
"$bin/masond" --managed --data-dir "$root/d" --config-dir "$root/c" --heartbeat-ms 100 --sweep-ms 3600000 --max-concurrent 4 --owner "$owner" --healthy-after-ms 2000 2>>"$root/masond.log" &
daemon=$!
until m status >/dev/null 2>&1; do kill -0 "$daemon" || { cat "$root/masond.log" >&2; exit 1; }; sleep 0.2; done
m self | head -1

echo "== 1. MASON lands a small change to its own source"
m task add "Note in the self-hosting guide" --project mason --explored --body "Add a line to docs/self-hosting.md." >/dev/null
wait_state 0 landed 300
git -C "$root/origin.git" log --oneline -3 main
git -C "$root/origin.git" show main:docs/self-hosting.md | tail -2
before=$(git -C "$root/origin.git" rev-parse main)

echo "== 2. build and prove the new main (this builds the repository in release mode)"
m update --check
echo "== 3. a 40-second agent session starts"
m task add "A long session" --project mason --explored --body "Slow work." >/dev/null
end=$((SECONDS + 60))
until ls "$root"/d/h/*/host.kv >/dev/null 2>&1 && grep -qh 'state=running' "$root"/d/h/*/host.kv 2>/dev/null || [ "$(m task show 1 --json | jq_state)" = developing ]; do
  [ $SECONDS -lt $end ] || { echo "the session never started" >&2; exit 1; }; sleep 0.5; done
sleep 3
pid_before=$(daemon_pid)
host_before=$(ls "$root"/d/h | head -1)
echo "daemon pid $pid_before, step host $host_before"

echo "== 4. MASON updates itself during that session"
m update
echo "== 5. the session was not interrupted"
[ "$(daemon_pid)" = "$pid_before" ] || { echo "the daemon's pid changed" >&2; exit 1; }
[ "$(ls "$root"/d/h | head -1)" = "$host_before" ] || { echo "a different step host" >&2; exit 1; }
echo "same daemon pid ($(daemon_pid)), same step host ($host_before)"
wait_state 1 landed 300
m task show 1 | head -8
m update status
echo "origin main:"
git -C "$root/origin.git" log --oneline -4 main
n=$(git -C "$root/origin.git" log --format=%s main | grep -c "change from task 1")
[ "$n" = 1 ] || { echo "task 1 is on main $n times" >&2; exit 1; }
[ "$(m update status --json | sed -n 's/.*"running": "\(.*\)",/\1/p')" = "$before" ] || echo "(running runtime is the build of main as of the update: $(m update status --json | sed -n 's/.*"running": "\(.*\)",/\1/p'))"
echo "s17-real OK"
