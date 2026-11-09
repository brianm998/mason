#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s98 exit test: finished step-host directories leave the registry. A real masond with the
# scripted agent and a grace period shortened to a few seconds. A task lands; its host
# directories move from h/ to t/<task>/hosts/; `mason logs` still shows them; a second task
# whose agent is still running keeps its hosts in h/, and `mason status` counts exactly those.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
command -v python3 >/dev/null || { echo "s98 needs python3" >&2; exit 1; }
root=$(mktemp -d /tmp/mason-s98.XXXXXX); daemon=""
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
fail() { set +e; echo "FAILED: $*" >&2; m task list >&2 2>&1; ls -R "$root/d/h" "$root/d/t" >&2 2>&1; tail -20 "$root/masond.log" >&2; exit 1; }
landed() { m task list --json | python3 -c "import json,sys; print(sum(1 for t in json.load(sys.stdin)['tasks'] if t.get('state')=='landed'))"; }
running() { m status --json | python3 -c "import json,sys; print(json.load(sys.stdin)['step_hosts_running'])"; }
landed_ge() { [ "$(landed)" -ge "$1" ]; }
running_is() { [ "$(running)" = "$1" ]; }
# Task ids start at 0: "one" is task 0 and "two" is task 1.
# Host directories of task $1 in h/ (the registry) and in t/$1/hosts/ (moved).
live() { { ls "$root/d/h" 2>/dev/null | grep "^t$1r" || true; } | wc -l | tr -d ' '; }
moved() { { ls "$root/d/t/$1/hosts" 2>/dev/null | grep "^t$1r" || true; } | wc -l | tr -d ' '; }
none_live() { [ "$(live "$1")" -eq 0 ]; }
wait_until() { # what seconds command...
  local what=$1 secs=$2; shift 2; local end=$((SECONDS + secs))
  until "$@"; do [ $SECONDS -lt $end ] || fail "$what"; sleep 0.2; done
}

echo "== setup: a repository; the agent of the second task keeps running"
mkdir -p "$root/c/projects" "$root/repo"
git init -q --bare -b main "$root/origin.git"
git clone -q "$root/origin.git" "$root/repo" 2>/dev/null
git -C "$root/repo" checkout -q -b main
echo base > "$root/repo/base.txt"
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

[suite.unit]
command = "sh $root/checks.sh"
parser = "tap"
min_tests = 1
level = "unit"
TOML
printf '#!/bin/sh\necho 1..1\necho "ok 1 - unit"\n' > "$root/checks.sh"
printf 'window = "1s"\n[[tier]]\nname = "batch"\nsuites = ["unit"]\n' > "$root/repo/.mason/landing.toml"
git -C "$root/repo" add -A
git -C "$root/repo" -c user.name=t -c user.email=t@t commit -qm init
git -C "$root/repo" push -q origin main
git -C "$root/repo" checkout -q -b owner-work
ln -s "$bin" "$root/bin"
cat > "$root/script.json" <<JSON
{"steps": {"develop": [[{"sh": "[ -e $root/slow ] && sleep 600; f=\$(mktemp ./f-XXXXXX) && echo x > \$f && git add -A && git -c user.name=t -c user.email=t@t commit -qm change"}, {"call": {"tool": "submit_change", "args": {"summary": "done", "head": "\$HEAD"}}}]]}}
JSON
cat > "$root/c/projects/mason.toml" <<TOML
path = "$root/repo"
base = "main"
remote = "origin"
test = "true"
claude_bin = "$bin/fake-claude"
step_timeout_s = 900
[agent_env]
FAKE_CLAUDE_SCRIPT = "$root/script.json"
TOML
# A grace period of 6 s instead of a day, and a sweep every 0.5 s.
MASON_HOST_GRACE_MS=6000 "$bin/masond" --port 0 --data-dir "$root/d" --config-dir "$root/c" --heartbeat-ms 100 --sweep-ms 500 --max-concurrent 4 --owner "$owner" 2>>"$root/masond.log" &
daemon=$!
until m status >/dev/null 2>&1; do kill -0 "$daemon" || { cat "$root/masond.log" >&2; exit 1; }; sleep 0.1; done

echo "== task one lands; its hosts are in the registry first"
m task add "one" --project mason --explored --body "x" >/dev/null
wait_until "task one did not land" 120 landed_ge 1
[ "$(live 0)" -ge 1 ] || [ "$(moved 0)" -ge 1 ] || fail "task one left no host directory"

echo "== the second task's agent keeps running"
touch "$root/slow"
m task add "two" --project mason --explored --body "x" >/dev/null
wait_until "task two's host never ran" 60 running_is 1

echo "== task one's host directories move out of the registry after the grace period"
wait_until "task one's hosts did not move" 60 none_live 0
[ "$(moved 0)" -ge 1 ] || fail "nothing in t/0/hosts"
ls "$root/d/h" | grep -q '^t0r.*\.in$' && fail "task one's input file stayed in h/"
echo "   moved: $(moved 0) entries"

echo "== mason logs still shows the moved run"
m logs 0 | grep -q '=== step run' || fail "mason logs 0 shows nothing for the moved run"

echo "== the running host stayed, and status counts it"
[ "$(live 1)" -ge 1 ] || fail "task two's host left the registry while its run is open"
[ "$(moved 1)" -eq 0 ] || fail "task two's host was moved while its run is open"
running_is 1 || fail "status counts $(running) running hosts, expected 1"

trap - EXIT; cleanup
echo "s98 OK"
