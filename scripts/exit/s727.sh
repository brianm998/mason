#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s727 exit test: a task chooses where its runs go (#699). Same setup as s722 (a real masond, a
# fake GitHub on loopback) with the global switch OFF throughout:
#  1. a task filed with `--run-on github` runs its suite remotely: the fake's origin got a push to
#     `mason/run/**` for it, and `mason task show` says "runs on: GitHub (task choice)";
#  2. a task filed without the flag runs here: no push, "runs on: this Mac (global switch)";
#  3. a held task filed with `--run-on github` and then set to `local` with
#     `mason task run-on` runs here when its turn comes: no push;
#  4. `run_on` is refused as a plain override and for a value other than github or local.
set -euo pipefail
trap 'rc=$?; [ "$rc" -eq 0 ] || echo "FAILED (exit $rc) at line $LINENO: $BASH_COMMAND" >&2' ERR
cd "$(git rev-parse --show-toplevel)"
root=$(mktemp -d /tmp/mason-s727.XXXXXX); daemon=""; gh=""
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
# Both suites are green here too: which one ran where is read from the pushes.
# A local run waits while "$root/hold" exists, so step 3 can keep task 1 from landing.
printf '#!/bin/sh\nwhile [ -e "%s/hold" ]; do sleep 0.1; done\necho 1..3\necho "ok 1 - a"\necho "ok 2 - b"\necho "ok 3 - c"\n' "$root" > "$root/repo/checks.sh"
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
{"steps": {"develop": [$step, $step, $step, $step, $step, $step]}}
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

echo "== the switch is off"
m remote status | grep -q '^mode local' || fail "remote is not local by default"
pushes() { awk '$3 ~ /^refs\/heads\/mason\/run\// && $2 !~ /^0+$/' "$root/pushes.log" | wc -l | tr -d ' '; }

echo "== 1. --run-on github runs remotely although the switch is off"
m task add "Chosen remote" --project alpha --body "x" --run-on github > /dev/null
m task show 0 | grep -q 'runs on: GitHub (task choice)' || fail "task show does not say GitHub (task choice)"
wait_state 0 landed 240
[ "$(pushes)" -ge 1 ] || fail "no push to mason/run/** for the task that chose GitHub"
m task show 0 | grep -q 'suite checks: on GitHub (ubuntu-latest), from the task' || fail "task show does not say checks went to GitHub from the task"

echo "== 2. no flag follows the switch"
n1=$(pushes)
touch "$root/hold"
m task add "Follows the switch" --project alpha --body "y" > /dev/null
m task show 1 | grep -q 'runs on: this Mac (global switch)' || fail "task show does not say this Mac (global switch)"

echo "== 3. run-on changes it later"
# Task 2 is filed and set to local while task 1 cannot land, so it is held throughout.
unlanded() { [ "$(m task show 1 --json | jq_state)" != landed ] || fail "task 1 landed $1, so task 2 was not held"; }
n2=$(pushes)
unlanded "before task 2 was filed"
m task add "Held" --project alpha --body "z" --run-on github --after 1 > /dev/null
m task show 2 | grep -q 'runs on: GitHub (task choice)' || fail "held task does not say GitHub"
m task run-on 2 local | grep -q 'this Mac (task choice)' || fail "run-on local did not take"
m task show 2 | grep -q 'runs on: this Mac (task choice)' || fail "task show after run-on local"
unlanded "before task 2 was set to local"
rm -f "$root/hold"
wait_state 1 landed 240
[ "$(pushes)" = "$n1" ] || fail "a task with no choice pushed a run"
wait_state 2 landed 240
[ "$(pushes)" = "$n2" ] || fail "a task set to local pushed a run"
m task run-on 2 default | grep -q 'this Mac (global switch)' || fail "run-on default did not remove the choice"

echo "== 4. refusals"
m task override 0 run_on github >/dev/null 2>&1 && fail "run_on was accepted as a plain override"
m task add "Bad" --project alpha --body "q" --run-on sideways >/dev/null 2>&1 && fail "--run-on sideways was accepted"
m task run-on 0 sideways >/dev/null 2>&1 && fail "run-on sideways was accepted"
echo "s727 passed"
