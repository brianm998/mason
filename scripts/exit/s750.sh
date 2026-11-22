#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s750 exit test: a task's test step that goes to GitHub takes no local `compile` or `test-suite`
# slot (#761). Same setup as s748, with both slots of size 1 held by `mason hold`:
#  1. remote on: the test step reaches the fake GitHub while the holds are held, and the task lands;
#  2. remote off: the test step waits for a slot and does not reach GitHub.
# (Debt payments take the same path, `pool_needs`; they are covered by the unit test only.)
set -euo pipefail
trap 'rc=$?; [ "$rc" -eq 0 ] || echo "FAILED (exit $rc) at line $LINENO: $BASH_COMMAND" >&2' ERR
cd "$(git rev-parse --show-toplevel)"
root=$(mktemp -d /tmp/mason-s750.XXXXXX); daemon=""; gh=""; holder=""
cleanup() {
  exec 3>&- 2>/dev/null || true
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
fail() { echo "FAILED: $*" >&2; set +e; echo "hold exit status: $(cat "$root/hold.rc" 2>/dev/null || echo running)" >&2; echo "hold stderr:" >&2; cat "$root/hold.err" >&2 2>/dev/null; echo "hold stdout:" >&2; cat "$root/hold.out" >&2 2>/dev/null; ps -o pid,stat,command -p "${holder:-0}" >&2 2>&1; m task show 0 >&2 2>&1; tail -30 "$root/masond.log" >&2; cat "$root/pushes.log" >&2 2>/dev/null; exit 1; }
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
uses = { compile = 1, test-suite = 1 }
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
pwd > hello.txt
mkdir -p .github/workflows .mason
printf 'name: evil\non: push\n' > .github/workflows/mason-run.yml
printf 'name: evil2\non: push\n' > .github/workflows/evil.yml
printf '{"command": "true"}' > .mason/run.json
git add -A; git -c user.name=t -c user.email=t@t commit -qm "work"
SH
step='[{"sh": "sh '"$root"'/agent.sh"}, {"call": {"tool": "submit_change", "args": {"summary": "added hello.txt", "head": "$HEAD"}}}]'
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

[remote_runs]
kind = "github-actions"
api = "http://127.0.0.1:$port"
repo = "o/r"
token = "env:FAKE_GITHUB_TOKEN"
runs_on = "macos-14"
TOML
export FAKE_GITHUB_TOKEN=recorded-test-token
start_daemon() {  # FORCE (1 or empty)
  MASON_TEST_FORCE_REMOTE="$1" "$bin/masond" --port 0 --data-dir "$root/d" --config-dir "$root/c" --heartbeat-ms 100 --sweep-ms 3600000 --owner "$owner" 2>>"$root/masond.log" &
  daemon=$!
  until m status >/dev/null 2>&1; do kill -0 "$daemon" || { cat "$root/masond.log" >&2; exit 1; }; sleep 0.1; done
}
hold_slots() {
  # stdin is a fifo kept open on fd 3 (a background job's own stdin is /dev/null, which ends the hold at once)
  rm -f "$root/hold.in" "$root/hold.out"; mkfifo "$root/hold.in"
  rm -f "$root/hold.err" "$root/hold.rc"
  ( m hold --needs compile,test-suite --pipeline other --label "other work" <"$root/hold.in" >"$root/hold.out" 2>"$root/hold.err"; echo $? > "$root/hold.rc" ) &
  holder=$!
  exec 3>"$root/hold.in"
  until grep -q granted "$root/hold.out" 2>/dev/null; do kill -0 "$holder" || fail "the hold ended"; sleep 0.1; done
}

echo "== 1. remote on: the check reaches GitHub with both slots held elsewhere"
start_daemon 1
hold_slots
m task add "Remote check" --project alpha --body "x" > /dev/null
end=$((SECONDS + 120))
until grep -q 'head_sha=' "$root/gh/requests.log" 2>/dev/null; do
  [ $SECONDS -lt $end ] || fail "the test step never reached GitHub while the slots were held"
  sleep 0.3
done
kill -0 "$holder" || fail "the held slots were released"
wait_state 0 landed 180
exec 3>&-; kill "$holder" 2>/dev/null || true; holder=""

echo "== 2. remote off: the same check waits for the compile slot"
kill -9 "$daemon"; daemon=""
: > "$root/gh/requests.log"
start_daemon ""
hold_slots
m task add "Local check" --project alpha --body "x" > /dev/null
end=$((SECONDS + 120))
until m task show 1 2>/dev/null | grep -qi 'compile'; do
  [ $SECONDS -lt $end ] || fail "the test step never showed it waits for compile"
  sleep 0.3
done
! grep -q 'head_sha=' "$root/gh/requests.log" || fail "a local test step reached GitHub"
echo "s750 passed"
