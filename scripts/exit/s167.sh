#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s167 exit test: a task that conflicts only with a batch-mate waits for the next batch, not develop.
# A real masond with a batched project (max_size 2): two tasks rewrite the same line of f.txt. The
# batch's landing suite fails the first time it runs, so the first task is sent back red; the other
# was left out of that batch for its conflict, lands in the next one, and `mason task show` lists a
# single developing run for it.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
root=$(mktemp -d /tmp/mason-s167.XXXXXX); daemon=""
cleanup() {
  [ -n "$daemon" ] && kill -9 "$daemon" 2>/dev/null || true
  for f in "$root"/d/h/*/host.kv; do
    [ -e "$f" ] || continue
    for k in host_pid child_pid; do pid=$(sed -n "s/^$k=//p" "$f"); kill -9 "-$pid" 2>/dev/null || kill -9 "$pid" 2>/dev/null || true; done
  done
  rm -rf "$root"
}
trap cleanup EXIT
cargo build -q --workspace --bins
bin=$PWD/target/debug
owner=${USER:-owner}
m() { "$bin/mason" --data-dir "$root/d" --config-dir "$root/c" "$@"; }
fail() { set +e; echo "FAILED: $*" >&2; m task list >&2 2>&1; tail -30 "$root/masond.log" >&2; exit 1; }
wait_until() { # what seconds command...
  local what=$1 secs=$2; shift 2; local end=$((SECONDS + secs))
  until "$@"; do [ $SECONDS -lt $end ] || fail "$what"; sleep 0.2; done
}
landed_count() { m task list --json | python3 -c "import json,sys; print(sum(1 for t in json.load(sys.stdin)['tasks'] if t.get('state')=='landed'))"; }
one_landed() { [ "$(landed_count)" -ge 1 ]; }
develops() { m task show "$1" --json | python3 -c "import sys; print(sys.stdin.read().count('\"step\": \"developing\"'))"; }

mkdir -p "$root/c/projects" "$root/repo"
git init -q --bare -b main "$root/origin.git"
git clone -q "$root/origin.git" "$root/repo" 2>/dev/null
git -C "$root/repo" checkout -q -b main
echo base > "$root/repo/f.txt"
mkdir -p "$root/repo/.mason"
cat > "$root/repo/.mason/mason.toml" <<TOML
schema = 1
start = "develop"

[step.develop]
type = "agent"
category = "developing"
next = "land"

[step.land]
type = "landing"
category = "landing"

[suite.gate]
command = "sh gate.sh"
parser = "tap"
min_tests = 1
level = "unit"
TOML
# Red the first time the landing suite runs, green after.
cat > "$root/repo/gate.sh" <<SH
#!/bin/sh
if [ -e "$root/gate-ran" ]; then echo 1..1; echo "ok 1 - gate"; else touch "$root/gate-ran"; echo 1..1; echo "not ok 1 - gate"; exit 1; fi
SH
printf 'window = "20s"\nmax_size = 2\n[[tier]]\nname = "batch"\nsuites = ["gate"]\n[on_red]\nladder = []\n' > "$root/repo/.mason/landing.toml"
git -C "$root/repo" add -A
git -C "$root/repo" -c user.name=t -c user.email=t@t commit -qm init
git -C "$root/repo" push -q origin main
git -C "$root/repo" checkout -q -b owner-work
# Each task rewrites the one line with its own worktree's name: the same line, different text.
turn='{"sh": "basename \"$PWD\" > f.txt && git add -A && git -c user.name=t -c user.email=t@t commit -qm change"}, {"call": {"tool": "submit_change", "args": {"summary": "done", "head": "$HEAD"}}}'
echo "{\"steps\": {\"develop\": [[$turn], [$turn]]}}" > "$root/script.json"
cat > "$root/c/projects/mason.toml" <<TOML
path = "$root/repo"
base = "main"
remote = "origin"
test = "true"
claude_bin = "$bin/fake-claude"
step_timeout_s = 300
[agent_env]
FAKE_CLAUDE_SCRIPT = "$root/script.json"
TOML
"$bin/masond" --port 0 --data-dir "$root/d" --config-dir "$root/c" --heartbeat-ms 100 --sweep-ms 3600000 --max-concurrent 2 --owner "$owner" 2>>"$root/masond.log" &
daemon=$!
until m status >/dev/null 2>&1; do kill -0 "$daemon" || { cat "$root/masond.log" >&2; exit 1; }; sleep 0.1; done

echo "== two tasks edit the same line; the first batch fails its landing suite"
m task add "one" --project mason --explored --body "x" >/dev/null
m task add "two" --project mason --explored --body "x" >/dev/null
wait_until "no task landed" 240 one_landed
echo "   a task landed in a later batch"

echo "== the one that landed was not sent back to develop"
landed_id=$(m task list --json | python3 -c "import json,sys; print([t['id'] for t in json.load(sys.stdin)['tasks'] if t.get('state')=='landed'][0])")
n=$(develops "$landed_id")
[ "$n" = 1 ] || fail "task $landed_id has $n developing runs, not 1"
echo "   task $landed_id: 1 developing run"

trap - EXIT; cleanup
echo "s167 OK"
