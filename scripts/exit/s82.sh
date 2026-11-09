#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s82 exit test: landing re-verifies a moved base outside the repository lock. A real masond with
# the scripted agent and a landing suite that sleeps. While the batch verifies, the base moves
# (a push). While the re-verification of the folded tree runs, a second task starts its step and
# its worktree appears (making it takes the repository lock, which the re-verification used to
# hold). The batch then lands with both changes.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
command -v python3 >/dev/null || { echo "s82 needs python3" >&2; exit 1; }
root=$(mktemp -d /tmp/mason-s82.XXXXXX); daemon=""
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
fail() { set +e; echo "FAILED: $*" >&2; cat "$root/suite.log" >&2; m task list >&2 2>&1; tail -20 "$root/masond.log" >&2; exit 1; }
landed() { m task list --json | python3 -c "import json,sys; print(sum(1 for t in json.load(sys.stdin)['tasks'] if t.get('state')=='landed'))"; }
# Runs of the suite in the integration worktree (the batch's), started and ended.
count() { local c; c=$(grep -c "^$1 .*/d/l/" "$root/suite.log" 2>/dev/null) || true; echo "${c:-0}"; }
batch_starts() { count start; }
batch_ends() { count end; }
starts_ge() { [ "$(batch_starts)" -ge "$1" ]; }
landed_ge() { [ "$(landed)" -ge "$1" ]; }
wait_until() { # what seconds command...
  local what=$1 secs=$2; shift 2; local end=$((SECONDS + secs))
  until "$@"; do [ $SECONDS -lt $end ] || fail "$what"; sleep 0.1; done
}

echo "== setup: a repository with a landing suite that sleeps when the tree holds slow.unit"
mkdir -p "$root/c/projects" "$root/repo"
git init -q --bare -b main "$root/origin.git"
git clone -q "$root/origin.git" "$root/repo" 2>/dev/null
git -C "$root/repo" checkout -q -b main
echo base > "$root/repo/base.txt"
mkdir -p "$root/repo/.mason"
cat > "$root/checks.sh" <<SH
#!/bin/sh
echo "start \$PWD" >> $root/suite.log
[ -e slow.unit ] && sleep 8
echo "end \$PWD" >> $root/suite.log
echo 1..1
echo "ok 1 - unit"
SH
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

[suite.unit]
command = "sh $root/checks.sh"
parser = "tap"
min_tests = 1
level = "unit"
TOML
printf 'window = "1s"\n[[tier]]\nname = "batch"\nsuites = ["unit"]\n' > "$root/repo/.mason/landing.toml"
git -C "$root/repo" add -A
git -C "$root/repo" -c user.name=t -c user.email=t@t commit -qm init
git -C "$root/repo" push -q origin main
git -C "$root/repo" checkout -q -b owner-work
# The first agent to start makes the slow tree; every agent commits one file of its own.
ln -s "$bin" "$root/bin"
cat > "$root/script.json" <<JSON
{"steps": {"develop": [[{"sh": "if mkdir $root/first 2>/dev/null; then touch slow.unit; fi; f=\$(mktemp ./f-XXXXXX) && echo x > \$f && git add -A && git -c user.name=t -c user.email=t@t commit -qm change"}, {"call": {"tool": "submit_change", "args": {"summary": "done", "head": "\$HEAD"}}}]]}}
JSON
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
"$bin/masond" --port 0 --data-dir "$root/d" --config-dir "$root/c" --heartbeat-ms 100 --sweep-ms 3600000 --max-concurrent 4 --owner "$owner" 2>>"$root/masond.log" &
daemon=$!
until m status >/dev/null 2>&1; do kill -0 "$daemon" || { cat "$root/masond.log" >&2; exit 1; }; sleep 0.1; done

echo "== task one: its batch starts verifying"
m task add "one" --project mason --explored --body "x" >/dev/null
wait_until "the batch never started verifying" 120 starts_ge 1

echo "== the base moves while the batch verifies"
git clone -q "$root/origin.git" "$root/pusher"
echo pushed > "$root/pusher/pushed.txt"
git -C "$root/pusher" add -A
git -C "$root/pusher" -c user.name=t -c user.email=t@t commit -qm "push to the base"
git -C "$root/pusher" push -q origin HEAD:main

echo "== the re-verification of the folded tree starts"
wait_until "the batch was never re-verified" 120 starts_ge 2
[ "$(batch_ends)" -eq 1 ] || fail "the re-verification started before the first verification ended"

echo "== meanwhile another task starts its step and its worktree appears"
m task add "two" --project mason --explored --body "x" >/dev/null
wait_until "task two's worktree did not appear during the re-verification" 5 test -e "$root/d/w/mason/1/.git"
[ "$(batch_ends)" -eq 1 ] || fail "the re-verification had already ended: the check proves nothing"
echo "   worktree appeared while the re-verification was still running"

echo "== the batch lands with both changes"
wait_until "task one did not land" 120 landed_ge 1
git -C "$root/origin.git" cat-file -e main:pushed.txt || fail "the pushed file is not in the base"
n=$(git -C "$root/origin.git" ls-tree --name-only main | grep -c '^f-' || true)
[ "$n" -ge 1 ] || fail "task one's change is not in the base"
git -C "$root/origin.git" cat-file -e main:slow.unit || fail "task one's tree is not in the base"
wait_until "task two did not land" 150 landed_ge 2

trap - EXIT; cleanup
echo "s82 OK"
