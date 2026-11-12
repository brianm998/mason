#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s152 exit test: a stalled run is woken. A real masond with the scripted agent and a project
# with `cont_after_s = 1`; its test command stops one of its own children and waits for it.
# The stall watcher sends SIGCONT, the task lands, and `mason health` counts one SIGCONT.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
command -v python3 >/dev/null || { echo "s152 needs python3" >&2; exit 1; }
root=$(mktemp -d /tmp/mason-s152.XXXXXX); daemon=""
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
fail() { set +e; echo "FAILED: $*" >&2; m task list >&2 2>&1; m health >&2 2>&1; tail -20 "$root/masond.log" >&2; exit 1; }
landed() { m task list --json | python3 -c "import json,sys; print(sum(1 for t in json.load(sys.stdin)['tasks'] if t.get('state')=='landed'))"; }
landed_ge() { [ "$(landed)" -ge "$1" ]; }
wait_until() { # what seconds command...
  local what=$1 secs=$2; shift 2; local end=$((SECONDS + secs))
  until "$@"; do [ $SECONDS -lt $end ] || fail "$what"; sleep 0.2; done
}

echo "== setup: a repository whose test command stops its own child"
mkdir -p "$root/c/projects" "$root/repo"
git init -q --bare -b main "$root/origin.git"
git clone -q "$root/origin.git" "$root/repo" 2>/dev/null
git -C "$root/repo" checkout -q -b main
echo base > "$root/repo/base.txt"
git -C "$root/repo" add -A
git -C "$root/repo" -c user.name=t -c user.email=t@t commit -qm init
git -C "$root/repo" push -q origin main
git -C "$root/repo" checkout -q -b owner-work
# The first run stops a child of its own and waits for it (it sits in state T until someone
# sends SIGCONT); a run after that finds the marker and passes at once.
cat > "$root/stop.sh" <<SH
#!/bin/sh
[ -e "$root/stopped-once" ] && exit 0
touch "$root/stopped-once"
sh -c 'kill -STOP \$\$; echo resumed'
SH
cat > "$root/script.json" <<JSON
{"steps": {"develop": [[{"sh": "f=\$(mktemp ./f-XXXXXX) && echo x > \$f && git add -A && git -c user.name=t -c user.email=t@t commit -qm change"}, {"call": {"tool": "submit_change", "args": {"summary": "done", "head": "\$HEAD"}}}]]}}
JSON
cat > "$root/c/projects/mason.toml" <<TOML
path = "$root/repo"
base = "main"
remote = "origin"
test = "sh $root/stop.sh"
claude_bin = "$bin/fake-claude"
step_timeout_s = 900
test_timeout_s = 600
cont_after_s = 1
[agent_env]
FAKE_CLAUDE_SCRIPT = "$root/script.json"
TOML
"$bin/masond" --port 0 --data-dir "$root/d" --config-dir "$root/c" --heartbeat-ms 100 --watch-tick-ms 200 --owner "$owner" 2>>"$root/masond.log" &
daemon=$!
until m status >/dev/null 2>&1; do kill -0 "$daemon" || { cat "$root/masond.log" >&2; exit 1; }; sleep 0.1; done

echo "== the task's test stops its own child; the watcher continues it and the task lands"
m task add "stops a child" --project mason --explored --body "x" >/dev/null
wait_until "the task did not land" 300 landed_ge 1

echo "== mason health counts one SIGCONT"
out=$(m health)
echo "$out" | grep "Stall watch"
echo "$out" | grep -q "Stall watch: 1 SIGCONT(s) sent" || fail "mason health does not count exactly one SIGCONT"
echo "$out" | grep -q ", 0 silent tree(s) killed" || fail "a tree was killed"

trap - EXIT; cleanup
echo "s152 OK"
