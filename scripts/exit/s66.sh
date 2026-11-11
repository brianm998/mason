#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Stage 66 exit test: a lost step host is recorded, run again once, and never a verdict.
# A real masond with the scripted agent (fake-claude); no model, no network.
#  1. SIGKILL the host of a running test step: the step is run again once on a fresh host
#     (id ends -host1), the task lands, and `mason task show` says what happened.
#  2. SIGKILL the hosts of two tasks' test steps at once: one alert says so, with the details.
# Only hosts this script watched start are killed, after their start string and command line match.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
root=$(mktemp -d /tmp/mason-s66.XXXXXX); daemon=""
stop_daemon() {
  if [ -n "$daemon" ]; then kill -9 "$daemon" 2>/dev/null || true; wait "$daemon" 2>/dev/null || true; fi
  daemon=""
}
kill_hosts() {
  # Verified before it is killed: a stale pid in a registry may belong to someone else now.
  for hd in "$root"/d/h; do target/debug/step-host-ctl reap "$hd" >/dev/null 2>&1; done
}
cleanup() { stop_daemon; kill_hosts; rm -rf "$root" 2>/dev/null || { sleep 1; rm -rf "$root"; }; }
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
    for d in $1; do [ -e "$d/journal" ] && grep -q started "$d/journal" && { echo "$d"; return 0; }; done
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
  "develop": [[
    {"sh": "echo hello > f-$(date +%s%N).txt && git add -A && git -c user.name=t -c user.email=t@t commit -qm hello"},
    {"call": {"tool": "submit_change", "args": {"summary": "added a file", "head": "$HEAD"}}}
  ]]
}}
JSON
# The test command: the first run of each task hangs (so there is a host to kill); a run that finds
# its marker passes.
cat > "$root/t.sh" <<TSH
m="$root/marker-\$(basename "\$PWD")"
[ -f "\$m" ] && { echo second run; exit 0; }
touch "\$m"; echo started; sleep 120
TSH
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
"$bin/masond" --data-dir "$root/d" --config-dir "$cdir" --heartbeat-ms 100 --sweep-ms 3600000 --owner "$owner" 2>>"$root/masond.log" &
daemon=$!
until m status >/dev/null 2>&1; do kill -0 "$daemon" || { cat "$root/masond.log" >&2; exit 1; }; sleep 0.1; done

echo "== 1. a killed test host: the step is run again once and the task lands"
m task add "first" --project p --body x >/dev/null
dir=$(wait_started "$root/d/h/t0r*-test" 120) || die "the test step of task 0 never started"
kill_host "$dir" || die "could not verify the host of $dir"
echo "   SIGKILLed the host in $(basename "$dir")"
wait_state 0 landed 120
ls -d "$root"/d/h/t0r*-test-host1 >/dev/null 2>&1 || die "no fresh host (-host1) was started"
echo "   re-run on a fresh host: $(basename "$(ls -d "$root"/d/h/t0r*-test-host1)")"
j=$(m task show 0 --json)
[ "$(pyget failures <<<"$j")" = 0 ] || die "a lost host counted as a failure: $j"
[ "$(pyget host_losses.0.rerun <<<"$j")" = true ] || die "the loss is not marked re-run: $j"
[ "$(pyget host_losses.1 <<<"$j")" = "" ] || die "more than one loss recorded: $j"
show=$(m task show 0)
grep -E "host lost .test. \(re-run once on a fresh host\): host pid [0-9]+ gone at [0-9:]+, killed by signal 9, no exit record" <<<"$show" \
  || { echo "$show" >&2; die "task show does not say what happened"; }
echo "   failures: 0, one loss, one re-run"
[ "$(m alerts | awk '/were lost at the same time/ {n++} END {print n+0}')" = 0 ] || die "a single loss raised the together alert"

echo "== 2. two hosts killed at once: one alert"
sleep 6   # more than the 5 seconds that make losses "at the same time", so part 1 stays apart
m task add "second" --project p --body x >/dev/null
m task add "third" --project p --body x >/dev/null
d1=$(wait_started "$root/d/h/t1r*-test" 120) || die "the test step of task 1 never started"
d2=$(wait_started "$root/d/h/t2r*-test" 120) || die "the test step of task 2 never started"
kill_host "$d1" & k1=$!
kill_host "$d2" & k2=$!
wait "$k1" "$k2" || die "could not verify and kill both hosts"
echo "   SIGKILLed the hosts in $(basename "$d1") and $(basename "$d2")"
wait_state 1 landed 120
wait_state 2 landed 120
end=$((SECONDS + 30)); n=0
until n=$(m alerts | awk '/were lost at the same time/ {n++} END {print n+0}'); [ "$n" = 1 ]; do
  [ $SECONDS -lt $end ] || { m alerts >&2; die "expected one together alert, saw $n"; }
  sleep 0.3
done
a=$(m alerts | grep "were lost at the same time")
case "$a" in
  *"2 step hosts were lost at the same time (tasks #1, #2): something killed them"*"host pid"*"host pid"*) echo "   $a" ;;
  *) die "the alert is not as expected: $a" ;;
esac
echo "s66 passed"
