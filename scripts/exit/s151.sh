#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s151 exit test: a suite with `pty = true` shows its current test while it runs. A real
# masond, a scripted agent, and a project whose xctest-parsed suite prints
# "Test Case 'Hang.test' started." and then blocks until a file appears.
#  1. `mason task show` carries the started line while the suite is still blocked
#     (the file does not exist yet: it is created only after the line is seen).
#  2. Once the file is created the suite goes green and the task lands.
# The pseudo-terminal wrapper is macOS-only (`script`); on Linux the same suite runs
# unwrapped, the line still shows (a shell's echo is unbuffered), and the script says so.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
root=$(mktemp -d /tmp/mason-s151.XXXXXX); daemon=""
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
fail() { set +e; echo "FAILED: $*" >&2; m task show 0 >&2 2>&1; tail -20 "$root/masond.log" >&2; exit 1; }
state() { m task show 0 --json 2>/dev/null | sed -n 's/^  "state": "\(.*\)",$/\1/p'; }
shows_line() { m task show 0 2>/dev/null | grep -q "Test Case 'Hang.test' started."; }
wait_until() { # what seconds command...
  local what=$1 secs=$2; shift 2; local end=$((SECONDS + secs))
  until "$@"; do [ $SECONDS -lt $end ] || fail "$what"; sleep 0.2; done
}
is_landed() { [ "$(state)" = landed ]; }

echo "== setup: a repository whose suite prints a started line, then blocks on $root/go"
mkdir -p "$root/c/projects" "$root/repo"
git init -q --bare -b main "$root/origin.git"
git clone -q "$root/origin.git" "$root/repo" 2>/dev/null
git -C "$root/repo" checkout -q -b main
mkdir -p "$root/repo/.mason"
cat > "$root/repo/.mason/mason.toml" <<TOML
schema = 1
start = "develop"

[step.develop]
type = "agent"
category = "developing"
next = "test"

[step.test]
type = "test"
category = "testing"
suites = ["xc"]
max_rounds = 1
on_red = "develop"
next = "land"

[step.land]
type = "landing"
category = "landing"

[suite.xc]
command = "sh $root/xc.sh"
parser = "xctest"
min_tests = 1
level = "unit"
pty = true
TOML
cat > "$root/xc.sh" <<SH
#!/bin/sh
echo "Test Case 'Hang.test' started."
while [ ! -f "$root/go" ]; do sleep 0.1; done
echo "Test Case 'Hang.test' passed (0.1 seconds)."
SH
printf 'mode = "direct"\n' > "$root/repo/.mason/landing.toml"
echo base > "$root/repo/base.txt"
git -C "$root/repo" add -A
git -C "$root/repo" -c user.name=t -c user.email=t@t commit -qm init
git -C "$root/repo" push -q origin main
cat > "$root/script.json" <<JSON
{"steps": {"develop": [[{"sh": "f=\$(mktemp ./f-XXXXXX) && echo x > \$f && git add -A && git -c user.name=t -c user.email=t@t commit -qm change"}, {"call": {"tool": "submit_change", "args": {"summary": "done", "head": "\$HEAD"}}}]]}}
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
"$bin/masond" --port 0 --data-dir "$root/d" --config-dir "$root/c" --heartbeat-ms 100 --sweep-ms 3600000 --owner "$owner" 2>>"$root/masond.log" &
daemon=$!
until m status >/dev/null 2>&1; do kill -0 "$daemon" || { cat "$root/masond.log" >&2; exit 1; }; sleep 0.1; done

echo "== 1. the started line is in 'mason task show' while the suite is blocked"
m task add "pty" --project mason --explored --body "x" >/dev/null
wait_until "task show never carried the started line" 120 shows_line
[ ! -e "$root/go" ] || fail "the file existed before the line was seen"
echo "   $(m task show 0 | grep "Test Case 'Hang.test' started." | sed 's/^ *//')"
[ "$(state)" != landed ] || fail "the task landed while the suite was blocked"

echo "== 2. the file appears: the suite goes green and the task lands"
touch "$root/go"
wait_until "the task did not land" 120 is_landed
if [ "$(uname)" = Darwin ]; then
  echo "   ran under script(1) on a pseudo-terminal"
else
  echo "   NOTE: not macOS, so the suite ran unwrapped (the wrapper is macOS-only)"
fi
trap - EXIT; cleanup
echo "s151 OK"
