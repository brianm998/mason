#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Stage 296 exit test: one command from a scratch repository to a landed proof task.
# A scratch repository with a bare origin, the fake agent, and a running masond that has never
# heard of the project. Then `mason init` with its answers as flags (stdin is not a terminal, so
# nothing is asked) and `--land push`: it writes the setup, merges it, pushes it, registers the
# project with the running daemon (no restart), files the proof task, and says how to open the
# monitor. No other command is run. The task lands; MASON-WORKS.md is then on the bare origin.
# The elapsed time is printed. It is not asserted on (no timing assertions).
# Also checked: without --land nothing is merged or pushed, even with a remote.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
root=$(mktemp -d /tmp/mason-s296.XXXXXX); daemon=""
stop_daemon() {
  if [ -n "$daemon" ]; then kill -9 "$daemon" 2>/dev/null || true; wait "$daemon" 2>/dev/null || true; fi
  daemon=""
  for f in "$root"/d/h/*/host.kv; do
    [ -e "$f" ] || continue
    hp=$(sed -n "s/^host_pid=//p" "$f"); cp=$(sed -n "s/^child_pid=//p" "$f")
    if [ -n "$hp" ] && ps -o command= -p "$hp" 2>/dev/null | grep -qF "step-host --dir $(dirname "$f")"; then kill -9 "-$cp" 2>/dev/null || true; kill -9 "$hp" 2>/dev/null || true; fi
  done
}
cleanup() { stop_daemon; rm -rf "$root"; }
trap cleanup EXIT

cargo build -q --workspace --bins
bin=$PWD/target/debug
LOG="$root/masond.log"
fail() { echo "FAIL: $*" >&2; [ -f "$LOG" ] && tail -20 "$LOG" >&2; exit 1; }

mk_repo() {  # NAME: a repository with a bare origin, main pushed
  git init -q --bare -b main "$root/$1-origin.git"
  mkdir -p "$root/$1"
  git -C "$root/$1" init -q -b main
  printf 'hello\n' > "$root/$1/README.md"
  git -C "$root/$1" add -A
  git -C "$root/$1" -c user.name=t -c user.email=t@t commit -qm init
  git -C "$root/$1" remote add origin "$root/$1-origin.git"
  git -C "$root/$1" push -q origin main
}
m() {  # REPO args...: mason run from inside the repository
  r=$1; shift
  (cd "$root/$r" && MASON_NO_OPEN=1 "$bin/mason" --data-dir "$root/d" --config-dir "$root/c" "$@")
}
mk_repo first; mk_repo second

# The fake agent: a wrapper that names its script, so the overlay needs no agent_env.
cat > "$root/agent.sh" <<SH
printf 'MASON landed this task.\n' > MASON-WORKS.md
git add -A; git -c user.name=t -c user.email=t@t commit -qm "prove MASON works"
SH
turn='[{"sh": "sh '"$root"'/agent.sh"}, {"call": {"tool": "submit_change", "args": {"summary": "added the file", "head": "$HEAD"}}}]'
echo '{"steps": {"develop": ['"$turn,$turn,$turn"']}}' > "$root/script.json"
printf '#!/bin/sh\nFAKE_CLAUDE_SCRIPT=%s exec %s "$@"\n' "$root/script.json" "$bin/fake-claude" > "$root/claude"
chmod +x "$root/claude"

# masond is already running and has never heard of either project.
mkdir -p "$root/c/projects" "$root/d"
"$bin/masond" --port 0 --data-dir "$root/d" --config-dir "$root/c" --heartbeat-ms 100 --sweep-ms 3600000 --max-concurrent 2 2>>"$LOG" &
daemon=$!
until m first status >/dev/null 2>&1; do kill -0 "$daemon" 2>/dev/null || fail "masond did not start"; sleep 0.1; done

echo "== 1. no --land: nothing is merged or pushed, even with a remote"
main_before=$(git -C "$root/first" rev-parse main); origin_before=$(git -C "$root/first-origin.git" rev-parse main)
out=$(m first init --yes --no-prove --suites skip --workflow solo --landing direct --agent-bin "$root/claude" --id first 2>&1) || { echo "$out" >&2; fail "init without --land failed"; }
[ "$(git -C "$root/first" rev-parse main)" = "$main_before" ] || fail "main moved without --land"
[ "$(git -C "$root/first-origin.git" rev-parse main)" = "$origin_before" ] || fail "origin moved without --land"
echo "$out" | grep -q 'Nothing was merged or pushed' || fail "no word that nothing was landed: $out"
echo "$out" | grep -q 'Registered first with the running masond' || fail "the running daemon was not told: $out"
echo "PASS: --land defaults to no"

echo "== 2. one command to a landed proof task"
start=$(date +%s)
out=$(m second init --yes --suites skip --workflow solo --landing direct --agent-bin "$root/claude" --id second --land push 2>&1) || { echo "$out" >&2; fail "init --land push failed"; }
echo "$out" | sed 's/^/  | /'
echo "$out" | grep -q 'Merged mason/setup into main' || fail "not merged"
echo "$out" | grep -q 'Pushed main' || fail "not pushed"
echo "$out" | grep -q 'Registered second with the running masond' || fail "not registered with the running daemon"
id=$(echo "$out" | sed -n 's/.*Filed task #\([0-9]*\).*/\1/p')
[ -n "$id" ] || fail "no proof task was filed"
echo "$out" | grep -q 'Monitor:' || fail "no word on the monitor"
echo "$out" | grep -q 'Where to change what you chose' || fail "no summary of where choices live"
end=$((SECONDS + 240)); state=""
until [ "$state" = landed ]; do
  state=$(m second task show "$id" --json 2>/dev/null | sed -n 's/^  "state": "\(.*\)",$/\1/p')
  [ "$state" != failed ] || { m second task show "$id" >&2; fail "the task failed"; }
  [ $SECONDS -lt $end ] || { m second task show "$id" >&2; fail "timed out; the task is $state"; }
  sleep 0.5
done
elapsed=$(( $(date +%s) - start ))
git -C "$root/second-origin.git" show main:MASON-WORKS.md | grep -qx 'MASON landed this task.' || fail "the landed change is not on origin/main"
git -C "$root/second-origin.git" show main:.mason/mason.toml >/dev/null || fail ".mason/ is not on origin/main"
echo "PASS: init, merge, push, register, proof task #$id landed; elapsed ${elapsed}s (not asserted on)"
echo "s296: all checks passed"
