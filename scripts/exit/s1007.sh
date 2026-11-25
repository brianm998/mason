#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1007 exit test: only the test step can be skipped (#1047).
# A real masond, a project with a review step and one task:
#  1. `mason task skip --step review` is refused and records no debt;
#  2. `mason task skip --step test` still records one.
set -euo pipefail
trap 'rc=$?; [ "$rc" -eq 0 ] || echo "FAILED (exit $rc) at line $LINENO: $BASH_COMMAND" >&2' ERR
cd "$(git rev-parse --show-toplevel)"
root=$(mktemp -d /tmp/mason-s1007.XXXXXX); daemon=""
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
fail() { echo "FAILED: $*" >&2; set +e; m task show 0 >&2 2>&1; m logs 0 >&2 2>&1; tail -30 "$root/masond.log" >&2; exit 1; }

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
next = "review"

[step.review]
type = "agent"
category = "review"
max_rounds = 2
on_red = "develop"
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

cat > "$root/c/projects/alpha.toml" <<TOML
path = "$root/repo"
base = "main"
remote = "origin"
test = "false"
claude_bin = "$bin/fake-claude"

[agent_env]
FAKE_CLAUDE_SCRIPT = "$root/script.json"
TOML
echo '{"steps": {"develop": [[{"sh": "sleep 600"}]]}}' > "$root/script.json"
"$bin/masond" --port 0 --data-dir "$root/d" --config-dir "$root/c" --heartbeat-ms 100 --sweep-ms 3600000 --owner "$owner" 2>>"$root/masond.log" &
daemon=$!
until m status >/dev/null 2>&1; do kill -0 "$daemon" || { cat "$root/masond.log" >&2; exit 1; }; sleep 0.1; done

echo "== a skip of review is refused and records nothing"
m task add "Add a thing" --project alpha --body "x" > /dev/null
if out=$(m task skip 0 --step review --reason "exit test" 2>&1); then fail "the review skip was accepted: $out"; fi
echo "$out" | grep -q 'only the test step can' || fail "unexpected error: $out"
m debt list | grep -q 'step:review' && fail "a debt was recorded"

echo "== a skip of the test step is recorded"
m task skip 0 --step test --reason "exit test" > /dev/null || fail "the test skip was refused"
m debt list | grep -q 'step:test' || fail "no debt for the test step"
echo "s1007 passed"
