#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Stage 25 exit test: device test environments, on the fake provider. A real masond reads
# [targets] provider = "fake" (a JSON inventory the script rewrites to plug devices in and out),
# so every device fact comes from the fake and no real simulator, emulator or phone is touched.
# xcrun, adb and emulator are replaced on PATH by tripwires that fail the script if anything
# calls them.
#  1. Leasing by capability: two android-emulator units and one ios-simulator unit. Two tasks hold
#     different emulators at once, a third waits with a place in line, an iOS task is not blocked,
#     each suite sees MASON_DEVICE of the right kind, and no device is held twice.
#  2. A missing device is a wait, not a failure: the activity says whose fault it is and names
#     device:ios-device, an `environment` alert is listed, and `mason devices` still means pairing.
#  3. Plugging a device in resumes the waiting task within one inventory read (5 s allowed at
#     poll_ms = 500), and it completes on that device.
#  4. An expired certificate in the suite output is environmental: a wait plus an alert, no
#     failure; fixed, the task completes.
#  5. Lifecycle order per lease is setup, health, suite, teardown, and teardown follows a red suite.
# A real simulator, emulator and phone are s25-real.sh / s25-manual.sh (not run here).
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
root=$(mktemp -d /tmp/mason-s25.XXXXXX); daemon=""
cleanup() {
  [ -n "$daemon" ] && kill -9 "$daemon" 2>/dev/null || true
  for f in "$root"/d/h/*/host.kv; do
    [ -e "$f" ] || continue
    hp=$(sed -n "s/^host_pid=//p" "$f"); cp=$(sed -n "s/^child_pid=//p" "$f"); if [ -n "$hp" ] && ps -o command= -p "$hp" 2>/dev/null | grep -qF "step-host --dir $(dirname "$f")"; then kill -9 "-$cp" 2>/dev/null || true; kill -9 "$hp" 2>/dev/null || true; fi
  done
  rm -rf "$root"
}
trap cleanup EXIT

cargo build -q --workspace --bins
bin=target/debug
fix=$PWD/scripts/exit/fixtures/s25

# Tripwires in front of PATH: a real device tool being called fails the run.
mkdir -p "$root/tripwire"
for t in xcrun adb emulator; do
  printf '#!/bin/sh\necho "%s $*" >> "%s/tripwire.log"\nexit 99\n' "$t" "$root" > "$root/tripwire/$t"
  chmod +x "$root/tripwire/$t"
done
export PATH="$root/tripwire:$PATH"

m() { "$bin/mason" --data-dir "$root/d" --config-dir "$root/c" "$@"; }
fail() { echo "FAIL: $*" >&2; tail -20 "$root/masond.log" >&2 || true; exit 1; }
jq_state() { sed -n 's/^  "state": "\(.*\)",$/\1/p'; }
activity() { m task show "$1" --json | sed -n 's/^  "activity": "\(.*\)",$/\1/p'; }
failures() { m task show "$1" --json | sed -n 's/^  "failures": \([0-9]*\),$/\1/p'; }
wait_state() {  # TASK STATE SECONDS
  local end=$((SECONDS + $3))
  until [ "$(m task show "$1" --json 2>/dev/null | jq_state)" = "$2" ]; do
    if [ $SECONDS -ge $end ]; then
      m task show "$1" >&2
      fail "timed out waiting for task $1 to be $2"
    fi
    sleep 0.2
  done
}
wait_for() {  # WHAT SECONDS COMMAND...
  local what=$1 end=$((SECONDS + $2)); shift 2
  until "$@"; do [ $SECONDS -lt $end ] || fail "timed out waiting for $what"; sleep 0.2; done
}
log_has() { grep -qx "$1" "$root/calls.log" 2>/dev/null; }
act_has() { activity "$1" | grep -q "$2"; }
alert_has() { m alerts 2>/dev/null | grep -q "$1"; }
plug() {  # ID:KIND ...   rewrite the inventory, atomically
  local out='{"devices":[' first=1 d
  for d in "$@"; do
    [ $first = 1 ] || out="$out,"; first=0
    out="$out{\"id\":\"${d%%:*}\",\"kind\":\"${d##*:}\",\"os\":\"14\",\"name\":\"${d%%:*}\"}"
  done
  printf '%s]}' "$out" > "$root/devices.tmp"; mv "$root/devices.tmp" "$root/devices.json"
}

# One repository; the scripted agent changes android/, ios/ or phone/ by the task's mode, which
# decides which suite (and so which device kind) the task needs.
mkdir -p "$root/b" "$root/gate" "$root/c/projects"
git init -q --bare -b main "$root/origin.git"
git clone -q "$root/origin.git" "$root/repo" 2>/dev/null
git -C "$root/repo" checkout -q -b main
echo scratch > "$root/repo/README"
git -C "$root/repo" add -A
git -C "$root/repo" -c user.name=t -c user.email=t@t commit -qm init
git -C "$root/repo" push -q origin main
git -C "$root/repo" checkout -q -b owner-work
cat > "$root/agent.sh" <<SH
n=\$(basename "\$PWD")
mode=\$(cat "$root/b/\$n" 2>/dev/null || echo android)
m=\${mode%red}; mkdir -p "\$m"; echo "\$n" > "\$m/f-\$n.txt"
[ "\$mode" = androidred ] && echo red > android/red.txt
git add -A; git -c user.name=t -c user.email=t@t commit -qm "work on \$n"
SH
turn='[{"sh": "sh '"$root"'/agent.sh"}, {"call": {"tool": "submit_change", "args": {"summary": "change", "head": "$HEAD"}}}]'
turns=$turn; for _ in 1 2 3 4 5 6 7 8; do turns="$turns,$turn"; done
echo '{"steps": {"develop": ['"$turns"']}}' > "$root/script.json"
cat > "$root/c/projects/mobile.toml" <<TOML
path = "$root/repo"
base = "main"
remote = "origin"
test = "false"
claude_bin = "$PWD/$bin/fake-claude"
[agent_env]
FAKE_CLAUDE_SCRIPT = "$root/script.json"
TOML
cat > "$root/definition.toml" <<TOML
schema = 1
start = "develop"

[step.develop]
type = "agent"
category = "developing"
next = "test"

[step.test]
type = "test"
category = "testing"
suites = ["emu", "sim", "phone"]
on_red = "develop"
max_rounds = 1
next = "land"

[step.land]
type = "landing"
category = "landing"

[suite.emu]
command = "sh tap.sh $root/calls.log $root/gate $root/redflag"
parser = "tap"
min_tests = 3
level = "unit"
env = "emulator"

[suite.sim]
command = "sh tap.sh $root/calls.log $root/gate"
parser = "tap"
min_tests = 3
level = "unit"
env = "simulator"

[suite.phone]
command = "sh cert.sh $root/calls.log $root/certflag"
parser = "tap"
min_tests = 3
level = "unit"
env = "iphone"

[environment.emulator]
type = "android-emulator"
[environment.simulator]
type = "ios-simulator"
[environment.iphone]
type = "ios-device"

[impact]
"android/**" = ["emu"]
"ios/**" = ["sim"]
"phone/**" = ["phone"]
TOML
git clone -q "$root/origin.git" "$root/pusher" 2>/dev/null
mkdir -p "$root/pusher/.mason"
cp "$root/definition.toml" "$root/pusher/.mason/mason.toml"
cp "$fix/suite-tap.sh" "$root/pusher/tap.sh"; cp "$fix/suite-cert.sh" "$root/pusher/cert.sh"
git -C "$root/pusher" add -A
git -C "$root/pusher" -c user.name=t -c user.email=t@t commit -qm "definition and suites"
git -C "$root/pusher" push -q origin HEAD:main

plug emu-a1:android-emulator emu-a2:android-emulator sim-i1:ios-simulator
printf '[targets]\nprovider = "fake"\nfake_file = "%s/devices.json"\nfake_log = "%s/calls.log"\npoll_ms = 500\n' "$root" "$root" > "$root/c/daemon.toml"
"$bin/masond" --port 0 --data-dir "$root/d" --config-dir "$root/c" --heartbeat-ms 100 --sweep-ms 3600000 --max-concurrent 4 --env-retry-ms 300 2>>"$root/masond.log" &
daemon=$!
until m status >/dev/null 2>&1; do kill -0 "$daemon" 2>/dev/null || fail "masond did not start"; sleep 0.1; done

next=0
add() {  # MODE TITLE   (the task's number is in $LAST)
  LAST=$next; next=$((next + 1))
  echo "$1" > "$root/b/$LAST"
  m task add "$2" --project mobile --body "x" > /dev/null
}

echo "== 1. devices are leased by capability"
touch "$root/gate/hold"
add android "Android one"; a0=$LAST
add android "Android two"; a1=$LAST
wait_for "two emulators to be held at once" 90 sh -c "grep -qx 'suite emu-a1' '$root/calls.log' 2>/dev/null && grep -qx 'suite emu-a2' '$root/calls.log'"
add android "Android three"; a2=$LAST
wait_for "the third task to wait with a place in line" 60 act_has "$a2" "in line"
echo "third: $(activity "$a2")"
act_has "$a2" "device:android-emulator" || fail "the wait does not name the pool"
add ios "iOS one"; i0=$LAST
wait_for "the iOS suite to start beside the waiting android task" 60 log_has "suite sim-i1"
act_has "$a2" "in line" || fail "the third android task stopped waiting"
echo "PASS: two android tasks hold different emulators, the third waits, the iOS task is not blocked"
m targets list | tee "$root/targets.txt"
grep -q 'ios-simulator.*sim-i1.*leased by' "$root/targets.txt" || fail "mason targets list shows no lease for sim-i1"
grep -q 'device:android-emulator *2 of 2 in use' "$root/targets.txt" || fail "pool usage not shown"
m targets list --json | grep -q '"leased_by"' || fail "targets --json has no leases"
m devices | grep -q "paired" || fail "mason devices no longer means pairing"
echo "PASS: mason targets list shows the inventory and leases; mason devices is still pairing"
starts=$(grep '^suite ' "$root/calls.log" | sort)
[ "$(echo "$starts" | wc -l | tr -d ' ')" = 3 ] || fail "expected three suite starts so far: $starts"
[ "$(echo "$starts" | uniq | wc -l | tr -d ' ')" = 3 ] || fail "one device was held by two tasks: $starts"
echo "$starts" | grep -q 'emu-a1' && echo "$starts" | grep -q 'emu-a2' && echo "$starts" | grep -q 'sim-i1' || fail "wrong kinds: $starts"
rm "$root/gate/hold"
for t in $a0 $a1 $a2 $i0; do wait_state "$t" landed 120; done
[ "$(grep -c '^suite emu-a' "$root/calls.log")" = 3 ] || fail "android suites did not all run on emulators"
[ "$(grep -c '^suite sim-i' "$root/calls.log")" = 1 ] || fail "the iOS suite did not run on the simulator"
echo "PASS: every suite saw MASON_DEVICE of its own kind; none shared a device"

echo "== 2. a missing device is a wait, not a failure"
add phone "Needs a phone"; p0=$LAST
wait_for "an environmental wait for a phone" 60 act_has "$p0" "environment problem"
echo "activity: $(activity "$p0")"
act_has "$p0" "not the task's fault" || fail "the wait blames the task"
act_has "$p0" "device:ios-device" || fail "the wait does not name what it waits for"
[ "$(failures "$p0")" = 0 ] || fail "a missing device counted as a failure"
[ "$(m task show "$p0" --json | jq_state)" != failed ] || fail "a missing device failed the task"
wait_for "the environment alert" 30 alert_has "environment"
m alerts | grep environment
echo "PASS: no device is a wait plus an alert, charged to no task"

echo "== 3. plugging a device in resumes the task"
: > "$root/calls.log"
plug emu-a1:android-emulator emu-a2:android-emulator sim-i1:ios-simulator phone-1:ios-device
t0=$SECONDS
wait_for "the task to start on the phone within one read" 5 log_has "suite phone-1"
echo "resumed after $((SECONDS - t0)) s"
wait_state "$p0" landed 60
[ "$(failures "$p0")" = 0 ] || fail "the wait left a failure behind"
echo "PASS: the task resumed within one inventory read and landed on phone-1"

echo "== 4. an expired certificate is the machine's fault"
touch "$root/certflag"
add phone "Signed build"; c0=$LAST
wait_for "a wait naming the expired certificate" 60 act_has "$c0" "expired-certificate"
echo "activity: $(activity "$c0")"
act_has "$c0" "not the task's fault" || fail "the wait blames the task"
[ "$(failures "$c0")" = 0 ] || fail "an expired certificate counted as a failure"
[ "$(m task show "$c0" --json | jq_state)" != failed ] || fail "an expired certificate failed the task"
alert_has "environment" || fail "no environment alert for the certificate"
rm "$root/certflag"
wait_state "$c0" landed 60
echo "PASS: an expired certificate is environmental; fixed, the task completes"

echo "== 5. lifecycle order, and teardown after a red suite"
: > "$root/calls.log"
touch "$root/redflag"
add androidred "Red on the emulator"; r0=$LAST
wait_state "$r0" failed 120
cat "$root/calls.log"
[ "$(sed -n 1p "$root/calls.log")" = "setup emu-a1" ] || fail "first call was not setup"
[ "$(sed -n 2p "$root/calls.log")" = "health emu-a1" ] || fail "second call was not health"
[ "$(sed -n 3p "$root/calls.log")" = "suite emu-a1" ] || fail "third call was not the suite"
[ "$(tail -1 "$root/calls.log")" = "teardown emu-a1" ] || fail "the last call was not teardown"
[ "$(grep -c '^setup' "$root/calls.log")" = "$(grep -c '^teardown' "$root/calls.log")" ] || fail "a setup has no teardown"
echo "PASS: setup, health, suite, teardown; teardown ran after the suite failed"

[ ! -s "$root/tripwire.log" ] || fail "a real device tool was called: $(cat "$root/tripwire.log")"
echo "PASS: no real xcrun, adb or emulator was called"
echo "s25: all checks passed"
