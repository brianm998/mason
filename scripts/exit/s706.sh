#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s706 exit test: a suite run on GitHub Actions (#697). A real masond with one project whose
# `[remote_runs]` points at a fake GitHub on loopback and whose suite is forced remote by the
# test-only switch MASON_TEST_FORCE_REMOTE. The suite's own command would fail if it ran here;
# the fake's artifact says three tests passed. Checks:
#  1. the origin received a push to `mason/run/**`, and its tree has the base branch's
#     mason-run.yml (not the task's edited copy), no other workflow, and a `.mason/run.json`
#     that matches the pinned definition;
#  2. the step's verdict and test count are the ones in the artifact (green, three tests: the
#     local command would have been red);
#  3. every ref the run pushed was deleted.
set -euo pipefail
trap 'rc=$?; [ "$rc" -eq 0 ] || echo "FAILED (exit $rc) at line $LINENO: $BASH_COMMAND" >&2' ERR
cd "$(git rev-parse --show-toplevel)"
root=$(mktemp -d /tmp/mason-s706.XXXXXX); daemon=""; gh=""
cleanup() {
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
uses = { compile = 1 }
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
TOML
export FAKE_GITHUB_TOKEN=recorded-test-token
MASON_TEST_FORCE_REMOTE=1 "$bin/masond" --port 0 --data-dir "$root/d" --config-dir "$root/c" --heartbeat-ms 100 --sweep-ms 3600000 --owner "$owner" 2>>"$root/masond.log" &
daemon=$!
until m status >/dev/null 2>&1; do kill -0 "$daemon" || { cat "$root/masond.log" >&2; exit 1; }; sleep 0.1; done

echo "== a task runs its test step"
m task add "Remote suite" --project alpha --body "x" > /dev/null
wait_state 0 landed 180

echo "== 1. the push, and what the runner was given"
pushed=$(awk '$3 ~ /^refs\/heads\/mason\/run\// && $2 !~ /^0+$/ { print $2 " " $3 }' "$root/pushes.log")
[ -n "$pushed" ] || fail "no push to mason/run/** reached the origin"
n=0
while read -r sha ref; do
  n=$((n + 1))
  echo "   $ref -> $sha"
  tree() { git --git-dir "$root/origin.git" "$@"; }
  # The first run is the test step's, before anything of the task has landed: its workflow is
  # exactly the base branch's, though the task's own tree edited it. (Once the task has landed
  # its edit is the base branch's, so a later run is only checked for what it must never have.)
  if [ "$n" = 1 ]; then
    [ "$(tree show "$sha:.github/workflows/mason-run.yml")" = "$(cat "$root/base-workflow.yml")" ] \
      || fail "$ref carries a workflow that is not the base branch's"
  fi
  [ "$(tree ls-tree -r --name-only "$sha" .github)" = ".github/workflows/mason-run.yml" ] \
    || fail "$ref carries another workflow file"
  tree show "$sha:.mason/run.json" > "$root/run.json"
  python3 -I - "$root/run.json" <<'PY' || fail "$ref: .mason/run.json does not match the pinned definition"
import json, sys
v = json.load(open(sys.argv[1]))
assert v["command"] == "sh checks.sh", v
assert v["suite"] == "checks", v
assert v["project"] == "alpha", v
assert v["runs_on"] == "ubuntu-latest", v
assert v["limit_s"] > 0, v
PY
done <<< "$pushed"
echo "   $n run(s) pushed"

echo "== 2. the verdict and the count are the artifact's"
m task show 0 | grep 'tests '
m task show 0 | grep -q 'tests     verified: checks: green' || fail "the step is not green from the artifact"
grep -q 'head_sha=' "$root/gh/requests.log" || fail "the fake never saw the run being looked up"
grep -q '/artifacts' "$root/gh/requests.log" || fail "the fake never saw the artifact being fetched"

echo "== 3. every ref was deleted"
left=$(git --git-dir "$root/origin.git" for-each-ref 'refs/heads/mason/run' --format='%(refname)')
[ -z "$left" ] || fail "refs left behind: $left"
deleted=$(awk '$3 ~ /^refs\/heads\/mason\/run\// && $2 ~ /^0+$/' "$root/pushes.log" | wc -l | tr -d ' ')
[ "$deleted" -ge "$n" ] || fail "$n ref(s) pushed but only $deleted deleted"
echo "s706 passed"
