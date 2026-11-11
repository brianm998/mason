#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s108 exit test: a step host frozen at launch raises the `launch-frozen` alert. A real masond whose
# --host-bin waits for a gate file before it runs the real host, with the threshold at 1.5 s:
# `mason alerts` names the launch and its task; once the gate opens the alert clears and the step runs.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
root=$(mktemp -d /tmp/mason-s108.XXXXXX); daemon=""
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
fail() { set +e; echo "FAILED: $*" >&2; m alerts >&2 2>&1; m task list >&2 2>&1; tail -20 "$root/masond.log" >&2; exit 1; }
wait_until() { # what seconds command...
  local what=$1 secs=$2; shift 2; local end=$((SECONDS + secs))
  until "$@"; do [ $SECONDS -lt $end ] || fail "$what"; sleep 0.1; done
}
frozen() { m alerts 2>&1 | grep -q 'launch-frozen\|frozen'; }
cleared() { ! m alerts 2>&1 | grep -q 'frozen'; }
landed() { m task list --json | python3 -c "import json,sys; sys.exit(0 if any(t.get('state')=='landed' for t in json.load(sys.stdin)['tasks']) else 1)"; }

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
TOML
printf 'window = "1s"\n[[tier]]\nname = "batch"\nsuites = []\n' > "$root/repo/.mason/landing.toml"
git -C "$root/repo" add -A
git -C "$root/repo" -c user.name=t -c user.email=t@t commit -qm init
git -C "$root/repo" push -q origin main
git -C "$root/repo" checkout -q -b owner-work
cat > "$root/script.json" <<JSON
{"steps": {"develop": [[{"sh": "echo x > f.txt && git add -A && git -c user.name=t -c user.email=t@t commit -qm change"}, {"call": {"tool": "submit_change", "args": {"summary": "done", "head": "\$HEAD"}}}]]}}
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
# A host binary that sleeps until the gate exists, then is the real host.
cat > "$root/slow-host" <<SH
#!/bin/sh
while [ ! -e "$root/gate" ]; do sleep 0.1; done
exec "$bin/step-host" "\$@"
SH
chmod +x "$root/slow-host"
MASON_LAUNCH_FROZEN_MS=1500 "$bin/masond" --port 0 --data-dir "$root/d" --config-dir "$root/c" --host-bin "$root/slow-host" --heartbeat-ms 100 --sweep-ms 3600000 --max-concurrent 2 --owner "$owner" 2>>"$root/masond.log" &
daemon=$!
until m status >/dev/null 2>&1; do kill -0 "$daemon" || { cat "$root/masond.log" >&2; exit 1; }; sleep 0.1; done

echo "== a task whose host does not come up"
m task add "one" --project mason --explored --body "x" >/dev/null
wait_until "no launch-frozen alert" 120 frozen
out=$(m alerts)
echo "$out" | grep -q 'tasks #0' || fail "the alert does not name task #0: $out"
echo "   $(echo "$out" | grep frozen | head -1)"

echo "== the host proceeds"
touch "$root/gate"
wait_until "the alert did not clear" 60 cleared
wait_until "the step did not run to a landing" 150 landed

trap - EXIT; cleanup
echo "s108 OK"
