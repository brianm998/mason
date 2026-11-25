#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s798 exit test: a fast-tracked task still has the tests it adds run (#812).
# A real masond and one task fast-tracked with `mason task skip --all`, whose scripted agent adds
# only a failing test (the code the test needs is never written):
#  1. its `run_check` reports red, naming the test;
#  2. the task does not land.
set -euo pipefail
trap 'rc=$?; [ "$rc" -eq 0 ] || echo "FAILED (exit $rc) at line $LINENO: $BASH_COMMAND" >&2' ERR
cd "$(git rev-parse --show-toplevel)"
root=$(mktemp -d /tmp/mason-s798.XXXXXX); daemon=""
cleanup() {
  [ -n "$daemon" ] && kill -9 "$daemon" 2>/dev/null || true
  for hd in "$root"/d/h; do target/debug/step-host-ctl reap "$hd" >/dev/null 2>&1; done
  rm -rf "$root"
}
trap cleanup EXIT
cargo build -q --workspace --bins
bin=$PWD/target/debug
owner=${USER:-owner}
m() { "$bin/mason" --data-dir "$root/d" --config-dir "$root/c" "$@"; }
jq_state() { sed -n 's/^  "state": "\(.*\)",$/\1/p'; }
# The first line names the state and the last log line: the runner's `cause s798:` summary keeps only that line.
fail() {
  set +e
  local state last
  state=$(m task show 0 --json 2>/dev/null | jq_state)
  last=$(m logs 0 2>/dev/null | grep -v '^[[:space:]]*$' | tail -n 1)
  echo "FAILED: $* (task state: ${state:-unknown}; last log line: ${last:-none})" >&2
  m task show 0 >&2 2>&1; m logs 0 >&2 2>&1; tail -30 "$root/masond.log" >&2; exit 1
}

echo "== setup: a repository with one suite that can run tests by name"
mkdir -p "$root/c/projects"
git init -q --bare -b main "$root/origin.git"
git clone -q "$root/origin.git" "$root/repo" 2>/dev/null
git -C "$root/repo" checkout -q -b main
mkdir -p "$root/repo/.mason" "$root/repo/src"
printf 'mode = "direct"\nwindow = "1s"\n' > "$root/repo/.mason/landing.toml"
cat > "$root/repo/.mason/mason.toml" <<'TOML'
schema = 1
start = "develop"

[resource.compile]
count = 1

[resource.test-suite]
count = 1

[step.develop]
type = "agent"
category = "developing"
next = "test"

[step.test]
type = "test"
category = "testing"
suites = ["checks"]
on_red = "develop"
max_rounds = 2
next = "land"

[step.land]
type = "landing"
category = "landing"

[suite.checks]
build = "true"
command = "echo RAN-FULL; echo 1..1; echo 'ok 1 - all'"
rerun = 'for n in {tests}; do echo "test m::$n ... FAILED"; done; exit 1'
parser = "tap"
min_tests = 1
level = "unit"
uses = { test-suite = 1 }

[binding.unit]
when = ["development", "test_step"]

[impact]
"src/**" = ["checks"]
TOML
echo scratch > "$root/repo/README"
git -C "$root/repo" add -A
git -C "$root/repo" -c user.name=t -c user.email=t@t commit -qm "base"
git -C "$root/repo" push -q origin main
git -C "$root/repo" fetch -q origin

cat > "$root/agent.sh" <<'SH'
mkdir -p src
printf '#[test]\nfn needs_the_code() {}\n' > src/a.rs
git add -A; git -c user.name=t -c user.email=t@t commit -qm "only a test"
SH
step='[{"sh": "sh '"$root"'/agent.sh"}, {"call": {"tool": "run_check", "args": {"name": "tests"}}}, {"call": {"tool": "submit_change", "args": {"summary": "only a test", "head": "$HEAD"}}}]'
cat > "$root/script.json" <<JSON
{"steps": {"develop": [$step, $step, $step, $step]}}
JSON
cat > "$root/c/projects/alpha.toml" <<TOML
path = "$root/repo"
base = "main"
remote = "origin"
test = "false"
claude_bin = "$bin/fake-claude"

[agent_env]
FAKE_CLAUDE_SCRIPT = "$root/script.json"
TOML
"$bin/masond" --port 0 --data-dir "$root/d" --config-dir "$root/c" --heartbeat-ms 100 --sweep-ms 3600000 --owner "$owner" 2>>"$root/masond.log" &
daemon=$!
until m status >/dev/null 2>&1; do kill -0 "$daemon" || { cat "$root/masond.log" >&2; exit 1; }; sleep 0.1; done

echo "== the fast-tracked task's run_check is red, naming the new test"
m task add "Add a test" --project alpha --body "x" > /dev/null
m task skip 0 --all --reason "exit test" > /dev/null
end=$((SECONDS + 240))
until m logs 0 2>&1 | grep -q 'needs_the_code'; do
  [ "$(m task show 0 --json 2>/dev/null | jq_state)" != landed ] || fail "the task landed with a failing test"
  [ $SECONDS -lt $end ] || fail "run_check never named the failing test"
  sleep 0.5
done
m logs 0 2>&1 | grep -q 'RED' || fail "run_check was not red"

echo "== the task does not land"
for _ in $(seq 1 100); do
  [ "$(m task show 0 --json 2>/dev/null | jq_state)" != landed ] || fail "the task landed with a failing test"
  sleep 0.3
done
echo "s798 passed"
