#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s722 exit test: where runs go (#698). A real masond with one project whose `[remote_runs]` points
# at a fake GitHub on loopback, and a test step with two suites, one of them `remote = false`.
#  1. `mason remote on`: the fake's origin got a push to `mason/run/**` for the first suite
#     only, and `mason task show` names both places with their reasons;
#  2. `mason remote off` then `on` keeps the comments and other keys of remote.toml;
#  3. after `mason remote off` the next task's round runs both suites here: the origin
#     received no further push.
set -euo pipefail
trap 'rc=$?; [ "$rc" -eq 0 ] || echo "FAILED (exit $rc) at line $LINENO: $BASH_COMMAND" >&2' ERR
cd "$(git rev-parse --show-toplevel)"
root=$(mktemp -d /tmp/mason-s722.XXXXXX); daemon=""; gh=""
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
suites = ["checks", "localonly"]
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

[suite.localonly]
command = "sh localonly.sh"
parser = "tap"
min_tests = 3
level = "unit"
remote = false
TOML
# Both suites are green here too: which one ran where is read from the pushes.
printf '#!/bin/sh\necho 1..3\necho "ok 1 - a"\necho "ok 2 - b"\necho "ok 3 - c"\n' > "$root/repo/checks.sh"
cp "$root/repo/checks.sh" "$root/repo/localonly.sh"
echo scratch > "$root/repo/README"
git -C "$root/repo" add -A
git -C "$root/repo" -c user.name=t -c user.email=t@t commit -qm "base"
git -C "$root/repo" push -q origin main
git -C "$root/repo" fetch -q origin
cp "$root/repo/.github/workflows/mason-run.yml" "$root/base-workflow.yml"
: > "$root/pushes.log"

cat > "$root/agent.sh" <<'SH'
echo hello > "hello-$$.txt"
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
"$bin/masond" --port 0 --data-dir "$root/d" --config-dir "$root/c" --heartbeat-ms 100 --sweep-ms 3600000 --owner "$owner" 2>>"$root/masond.log" &
daemon=$!
until m status >/dev/null 2>&1; do kill -0 "$daemon" || { cat "$root/masond.log" >&2; exit 1; }; sleep 0.1; done

echo "== the switch is off by default; turn it on"
m remote status | grep -q '^default local' || fail "remote is not local by default"
m remote on > /dev/null
m remote status | grep -q '^default github' || fail "remote on did not take"
grep -q "^changed_by" "$root/c/remote.toml" || fail "remote.toml does not record who"

echo "== 1. a task's test step with two suites"
m task add "Remote suite" --project alpha --body "x" > /dev/null
wait_state 0 landed 240
pushed=$(awk '$3 ~ /^refs\/heads\/mason\/run\// && $2 !~ /^0+$/ { print $2 " " $3 }' "$root/pushes.log")
[ -n "$pushed" ] || fail "no push to mason/run/** reached the origin"
n=0
while read -r sha ref; do
  n=$((n + 1))
  git --git-dir "$root/origin.git" show "$sha:.mason/run.json" > "$root/run.json"
  python3 -I - "$root/run.json" <<'PY' || fail "$ref: a suite other than checks was pushed"
import json, sys
v = json.load(open(sys.argv[1]))
assert v["suite"] == "checks", v
PY
done <<< "$pushed"
echo "   $n run(s) pushed, all for checks"
shown=$(m task show 0)
echo "$shown" | grep 'where'
echo "$shown" | grep -q 'suite checks: on GitHub (ubuntu-latest), from the global switch' || fail "task show does not say checks went to GitHub"
echo "$shown" | grep -q 'suite localonly: on this Mac: the suite says remote = false' || fail "task show does not say why localonly stayed"
m config show --project alpha | grep -q 'localonly .*remote = false' || fail "config show does not say localonly's place"

echo "== 2. off and on again keep the comments and other keys"
printf '# my note\nextra = 1 # keep this\n' | cat - "$root/c/remote.toml" > "$root/remote.new" && mv "$root/remote.new" "$root/c/remote.toml"
m remote off > /dev/null
m remote on > /dev/null
m remote off > /dev/null
grep -q '^# my note' "$root/c/remote.toml" || fail "a comment was lost"
grep -q '^extra = 1 # keep this' "$root/c/remote.toml" || fail "another key was lost"
grep -q '^default = "local"' "$root/c/remote.toml" || fail "remote off did not take"
[ "$(wc -l < "$root/c/remote-changes.log")" -ge 4 ] || fail "changes are not recorded"

echo "== 3. with remote off the next round runs both suites here"
before=$(wc -l < "$root/pushes.log")
m task add "Local again" --project alpha --body "y" > /dev/null
wait_state 1 landed 240
after=$(wc -l < "$root/pushes.log")
# The task's own branch is pushed when it lands; only mason/run/** refs count.
newrun=$(tail -n +$((before + 1)) "$root/pushes.log" | awk '$3 ~ /^refs\/heads\/mason\/run\//' | wc -l | tr -d ' ')
[ "$newrun" = 0 ] || fail "$newrun push(es) to mason/run/** after remote off"
m task show 1 | grep -q 'suite checks: on this Mac: the global switch is off' || fail "task show does not say checks stayed here"
echo "s722 passed"
