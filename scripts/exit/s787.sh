#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s787 exit test: a develop agent's wait on GitHub is credited to its deadline, and a timeout
# note counts from the step's launch (#799). A real masond, a fake GitHub, and a develop step
# with a 10 s budget whose run_check goes remote (MASON_TEST_FORCE_REMOTE):
#  1. the fake GitHub holds the run queued for 20 s (twice the budget): the step is not failed
#     for time, `mason task show` names the GitHub wait, and the task lands once the run goes;
#  2. a step that does time out, after a `kill -9` and restart of the daemon mid-step, reports
#     its elapsed time from its launch (at least the budget) and says the daemon restarted.
set -euo pipefail
trap 'rc=$?; [ "$rc" -eq 0 ] || echo "FAILED (exit $rc) at line $LINENO: $BASH_COMMAND" >&2' ERR
cd "$(git rev-parse --show-toplevel)"
root=$(mktemp -d /tmp/mason-s787.XXXXXX); daemon=""; gh=""; holder=""
cleanup() {
  [ -n "$holder" ] && kill "$holder" 2>/dev/null || true
  [ -n "$daemon" ] && kill -9 "$daemon" 2>/dev/null || true
  [ -n "$gh" ] && kill -9 "$gh" 2>/dev/null || true
  for hd in "$root"/d/h; do target/debug/step-host-ctl reap "$hd" >/dev/null 2>&1; done
  rm -rf "$root"
}
trap cleanup EXIT
cargo build -q --workspace --bins
bin=$PWD/target/debug
owner=${USER:-owner}
m() { "$bin/mason" --data-dir "$root/d" --config-dir "$root/c" "$@"; }
jq_state() { sed -n 's/^  "state": "\(.*\)",$/\1/p'; }
fail() { echo "FAILED: $*" >&2; set +e; m task show 0 >&2 2>&1; tail -30 "$root/masond.log" >&2; cat "$root/pushes.log" >&2 2>/dev/null; exit 1; }
wait_state() {  # TASK STATE SECONDS
  local end=$((SECONDS + $3))
  until [ "$(m task show "$1" --json 2>/dev/null | jq_state)" = "$2" ]; do
    [ $SECONDS -lt $end ] || fail "timed out waiting for task $1 to be $2"
    sleep 0.2
  done
}

echo "== setup: a repository whose base branch has the workflow, a fake GitHub with an artifact"
mkdir -p "$root/gh" "$root/c/projects"
echo '[]' > "$root/gh/issues.json"; echo '{}' > "$root/gh/deps.json"
git init -q --bare -b main "$root/origin.git"
# Every push is logged, so the script can tell what the origin was sent and when it was deleted.
cat > "$root/origin.git/hooks/pre-receive" <<HOOK
#!/bin/sh
while read old new ref; do echo "\$old \$new \$ref" >> "$root/pushes.log"; done
HOOK
chmod +x "$root/origin.git/hooks/pre-receive"
echo "$root/origin.git" > "$root/gh/origin-path"
python3 -I - "$root/gh/run-artifact.zip" <<'PY'
import sys, zipfile
z = zipfile.ZipFile(sys.argv[1], "w")
z.writestr("output.log", "1..3\nok 1 - one\nok 2 - two\nok 3 - three\n")
z.writestr("result.json", '{"code": 0, "signal": 0, "build_s": 1, "run_s": 2}')
z.close()
PY
"$bin/fake-github" "$root/gh" "$root/gh.port" 2>"$root/gh.log" & gh=$!
until [ -s "$root/gh.port" ]; do sleep 0.05; done
port=$(cat "$root/gh.port")

git clone -q "$root/origin.git" "$root/repo" 2>/dev/null
git -C "$root/repo" checkout -q -b main
mkdir -p "$root/repo/.mason" "$root/repo/.github/workflows"
cp templates/github/mason-run.yml "$root/repo/.github/workflows/mason-run.yml"
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
command = "sh checks.sh"
parser = "tap"
min_tests = 3
level = "unit"
build = "true"
uses = { test-suite = 1 }
TOML
# The suite's own command is red: only the fake's artifact can make the step green.
printf '#!/bin/sh\necho 1..1\necho "not ok 1 - ran here"\nexit 1\n' > "$root/repo/checks.sh"
echo scratch > "$root/repo/README"
git -C "$root/repo" add -A
git -C "$root/repo" -c user.name=t -c user.email=t@t commit -qm "base"
git -C "$root/repo" push -q origin main
git -C "$root/repo" fetch -q origin
cp "$root/repo/.github/workflows/mason-run.yml" "$root/base-workflow.yml"
: > "$root/pushes.log"

# The scripted agent edits the workflow to report green and adds one of its own: neither may
# reach the runner.
cat > "$root/agent.sh" <<SH
echo hello > hello.txt
mkdir -p .github/workflows .mason
printf 'name: evil\non: push\n' > .github/workflows/mason-run.yml
printf 'name: evil2\non: push\n' > .github/workflows/evil.yml
printf '{"command": "true"}' > .mason/run.json
git add -A; git -c user.name=t -c user.email=t@t commit -qm "work"
SH
step='[{"sh": "sh '"$root"'/agent.sh"}, {"call": {"tool": "run_check", "args": {"name": "tests"}}}, {"call": {"tool": "submit_change", "args": {"summary": "added hello.txt", "head": "$HEAD"}}}]'
cat > "$root/script.json" <<JSON
{"steps": {"develop": [$step, $step, $step, $step]}}
JSON
cat > "$root/c/projects/alpha.toml" <<TOML
path = "$root/repo"
base = "main"
remote = "origin"
test = "false"
step_timeout_s = 10
claude_bin = "$bin/fake-claude"

[agent_env]
FAKE_CLAUDE_SCRIPT = "$root/script.json"

[remote_runs]
kind = "github-actions"
api = "http://127.0.0.1:$port"
repo = "o/r"
token = "env:FAKE_GITHUB_TOKEN"
runs_on = "macos-14"
TOML
export FAKE_GITHUB_TOKEN=recorded-test-token
start_daemon() {
  MASON_TEST_FORCE_REMOTE=1 "$bin/masond" --port 0 --data-dir "$root/d" --config-dir "$root/c" --heartbeat-ms 100 --sweep-ms 3600000 --owner "$owner" 2>>"$root/masond.log" &
  daemon=$!
  until m status >/dev/null 2>&1; do kill -0 "$daemon" || { cat "$root/masond.log" >&2; exit 1; }; sleep 0.1; done
}

echo "== 1. a run held queued past the budget does not fail the step; the wait is named"
touch "$root/gh/run-hold"
start_daemon
m task add "Slow runner" --project alpha --body "x" > /dev/null
end=$((SECONDS + 120))
until grep -q 'head_sha=' "$root/gh/requests.log" 2>/dev/null; do
  [ $SECONDS -lt $end ] || fail "the check never reached GitHub"
  sleep 0.3
done
sleep 20
[ "$(m task show 0 --json | jq_state)" = developing ] || fail "the step did not stay in develop while the run waited"
m task show 0 | grep -qi 'waiting on GitHub' || fail "task show does not name the GitHub wait"
m task show 0 | grep -qi 'ran out of time' && fail "the step was stopped for time while it waited on GitHub"
rm "$root/gh/run-hold"
wait_state 0 landed 180

echo "== 2. a step that times out after a daemon restart counts from its launch"
step2='[{"sh": "sleep 300"}]'
cat > "$root/script.json" <<JSON
{"steps": {"develop": [$step2, $step2, $step2, $step2]}}
JSON
m task add "Times out" --project alpha --body "y" > /dev/null
sleep 6
kill -9 "$daemon"; daemon=""
start_daemon
end=$((SECONDS + 120))
until m task show 1 2>/dev/null | grep -q 'ran out of time'; do
  [ $SECONDS -lt $end ] || fail "the step never ran out of time"
  sleep 0.5
done
note=$(m task show 1 | grep -o 'the step ran out of time: 0h[0-9]*m[0-9]*s' | head -1)
secs=$(echo "$note" | sed -n 's/.*0h\([0-9]*\)m\([0-9]*\)s/\1 \2/p' | awk '{print $1*60+$2}')
[ -n "$secs" ] && [ "$secs" -ge 10 ] || fail "elapsed was not counted from the launch: '$note'"
m task show 1 | grep -q 'daemon restarted during the step' || fail "the note does not name the restart"
echo "s787 passed"
