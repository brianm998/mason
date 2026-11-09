#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Stage 83 exit test: a lost step host does not fail an agent step; it is resumed.
# A real masond with the scripted agent (fake-claude); no model, no network.
# SIGKILL the host of the running develop step: the step continues (resumed with
# `--resume`), the task lands, `mason task show` names the loss, `mason alerts` shows no failure.
# Only a host this script watched start is killed, after its start string and command line match.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
root=$(mktemp -d /tmp/mason-s83.XXXXXX); daemon=""
stop_daemon() {
  if [ -n "$daemon" ]; then kill -9 "$daemon" 2>/dev/null || true; wait "$daemon" 2>/dev/null || true; fi
  daemon=""
}
kill_hosts() {
  # Verified before it is killed: a stale pid in a registry may belong to someone else now.
  for hd in "$root"/d/h; do target/debug/step-host-ctl reap "$hd" >/dev/null 2>&1; done
}
cleanup() { stop_daemon; kill_hosts; rm -rf "$root"; }
trap cleanup EXIT
cargo build -q --workspace --bins
bin=target/debug
owner=${USER:-owner}
cdir="$root/c"; mkdir -p "$cdir/projects"
m() { "$bin/mason" --data-dir "$root/d" --config-dir "$cdir" "$@"; }
pyget() { python3 -I -c 'import json,sys; v=json.load(sys.stdin)
for k in sys.argv[1].split("."):
    v = (v[int(k)] if int(k) < len(v) else None) if isinstance(v, list) else v.get(k) if isinstance(v, dict) else None
print("" if v is None else (json.dumps(v) if isinstance(v,(dict,list,bool)) else v))' "$1"; }
die() { echo "$1" >&2; tail -20 "$root/masond.log" >&2; exit 1; }
wait_state() {  # TASK STATE SECONDS
  local end=$((SECONDS + $3))
  until [ "$(m task show "$1" --json 2>/dev/null | pyget state)" = "$2" ]; do
    [ $SECONDS -lt $end ] || { m task show "$1" >&2; die "timed out waiting for task $1 to be $2"; }
    sleep 0.3
  done
}
# kill_host DIR: SIGKILL the host (not the command) if it still is the process the registry recorded
kill_host() {
  local dir=$1 pid start
  pid=$(sed -n 's/^host_pid=//p' "$dir/host.kv"); start=$(sed -n 's/^host_start=//p' "$dir/host.kv")
  [ -n "$pid" ] && [ -n "$start" ] || return 1
  [ "$(ps -o lstart= -p "$pid" | sed 's/^ *//;s/ *$//')" = "$start" ] || return 1
  ps -o command= -p "$pid" | grep -qF "$dir" || return 1
  kill -9 "$pid"
}
wait_started() {  # GLOB SECONDS: wait for the test host's command to print `started`; print its dir
  local end=$((SECONDS + $2)) d
  while [ $SECONDS -lt $end ]; do
    for d in $1; do [ -e "$d/journal" ] && grep -q interrupt-me "$d/journal" && { echo "$d"; return 0; }; done
    sleep 0.1
  done
  return 1
}

repo="$root/r"; origin="$root/o.git"
git init -q --bare -b main "$origin" 2>/dev/null
git clone -q "$origin" "$repo" 2>/dev/null
git -C "$repo" checkout -q -b main
mkdir -p "$repo/.mason"
cat > "$repo/.mason/mason.toml" <<'TOML'
schema = 1
start = "explore"

[step.explore]
type = "agent"
category = "exploring"
next = "develop"

[step.develop]
type = "agent"
category = "developing"
next = "test"

[step.test]
type = "test"
category = "testing"
max_rounds = 2
on_red = "develop"
next = "land"

[step.land]
type = "landing"
category = "landing"
TOML
printf 'mode = "direct"\n' > "$repo/.mason/landing.toml"
echo base > "$repo/README"
git -C "$repo" add -A
git -C "$repo" -c user.name=t -c user.email=t@t commit -qm pipeline
git -C "$repo" push -q origin main
git -C "$repo" checkout -q -b owner-work

cat > "$root/script.json" <<'JSON'
{"steps": {
  "explore": [[{"call": {"tool": "submit_plan", "args": {"summary": "add a file", "files": ["f.txt"], "approach": "write it", "acceptance_criteria": ["f.txt exists"]}}}]],
  "develop": [
    [{"say": "interrupt-me"}, {"sh": "sleep 120"}],
    [
      {"sh": "echo hello > f-$(date +%s%N).txt && git add -A && git -c user.name=t -c user.email=t@t commit -qm hello"},
      {"call": {"tool": "submit_change", "args": {"summary": "added a file", "head": "$HEAD"}}}
    ]
  ]
}}
JSON
# The test command passes at once: only the develop host is killed.
printf 'echo tests ok\n' > "$root/t.sh"
cat > "$cdir/projects/p.toml" <<TOML
path = "$repo"
base = "main"
remote = "origin"
test = "sh $root/t.sh"
claude_bin = "$PWD/$bin/fake-claude"
[agent_env]
FAKE_CLAUDE_SCRIPT = "$root/script.json"
TOML
: > "$root/masond.log"
"$bin/masond" --port 0 --data-dir "$root/d" --config-dir "$cdir" --heartbeat-ms 100 --sweep-ms 3600000 --owner "$owner" 2>>"$root/masond.log" &
daemon=$!
until m status >/dev/null 2>&1; do kill -0 "$daemon" || { cat "$root/masond.log" >&2; exit 1; }; sleep 0.1; done

echo "== a killed develop host: the step is resumed, the task lands, no failure"
m task add "first" --project p --body x >/dev/null
dir=$(wait_started "$root/d/h/t0r*-[0-9]" 120) || die "the develop session of task 0 never started"
kill_host "$dir" || die "could not verify the host of $dir"
echo "   SIGKILLed the host in $(basename "$dir")"
wait_state 0 landed 120
j=$(m task show 0 --json)
[ "$(pyget failures <<<"$j")" = 0 ] || die "a lost host counted as a failure: $j"
[ "$(pyget failure <<<"$j")" = "" ] || die "the task has a failure: $j"
[ "$(pyget host_losses.0.step <<<"$j")" = develop ] || die "the loss is not recorded against develop: $j"
[ "$(pyget host_losses.1 <<<"$j")" = "" ] || die "more than one loss recorded: $j"
show=$(m task show 0)
grep -E "host lost .develop. \(.*\): host pid [0-9]+ gone at [0-9:]+, killed by signal 9, no exit record" <<<"$show" \
  || { echo "$show" >&2; die "task show does not say what happened"; }
grep -q -- "--resume" "$root/script.json.argv" || die "the step was not resumed"
if m alerts | grep -qi "failed"; then m alerts >&2; die "alerts show a failure"; fi
echo "s83 passed"
