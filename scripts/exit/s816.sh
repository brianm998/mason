#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s816 exit test: the placement mode (#831). A real masond with two projects: alpha has a
# `[remote_runs]` (and a fake GitHub on loopback), beta has none. The global mode is set to
# `remote`, alpha's to `local` and one task of alpha's to `remote`. Then `mason remote status
# --json` and `mason task show --json` say:
#  1. alpha is local with source `project`; beta is local because it has no `[remote_runs]`;
#  2. another task of alpha is local (from the project); the chosen task is remote (from the task);
#  3. the old values still read (`mason remote on` is `remote`, `run-on github` is `remote`);
#  4. a bad mode is refused. (A project without `[remote_runs]` took none until #1150; it now
#     takes one and is nodes only, which s1123 covers.)
set -euo pipefail
trap 'rc=$?; [ "$rc" -eq 0 ] || echo "FAILED (exit $rc) at line $LINENO: $BASH_COMMAND" >&2' ERR
cd "$(git rev-parse --show-toplevel)"
root=$(mktemp -d /tmp/mason-s816.XXXXXX); daemon=""; gh=""
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
printf '#!/bin/sh\necho 1..3\necho "ok 1 - a"\necho "ok 2 - b"\necho "ok 3 - c"\n' > "$root/repo/checks.sh"
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
# beta: its own clone of the same repository and no [remote_runs].
git clone -q "$root/origin.git" "$root/repo-beta" 2>/dev/null
cat > "$root/c/projects/beta.toml" <<TOML
path = "$root/repo-beta"
base = "main"
remote = "origin"
test = "false"
claude_bin = "$bin/fake-claude"

[agent_env]
FAKE_CLAUDE_SCRIPT = "$root/script.json"
TOML
export FAKE_GITHUB_TOKEN=recorded-test-token
"$bin/masond" --port 0 --data-dir "$root/d" --config-dir "$root/c" --heartbeat-ms 100 --sweep-ms 3600000 --owner "$owner" 2>>"$root/masond.log" &
daemon=$!
until m status >/dev/null 2>&1; do kill -0 "$daemon" || { cat "$root/masond.log" >&2; exit 1; }; sleep 0.1; done


jf() {  # JSON FILTER: a python expression over the parsed document `j`
  python3 -I -c 'import json,sys; j=json.load(sys.stdin); print(eval(sys.argv[1]))' "$1"
}
proj() {  # PROJECT FIELD
  m remote status --json | jf "[p for p in j['projects'] if p['project']=='$1'][0]['$2']"
}
runon() {  # TASK FIELD
  m task show "$1" --json | jf "j['run_on']['$2']"
}

echo "== set the three levels"
m remote mode remote >/dev/null
[ "$(m remote status --json | jf "j['mode']")" = remote ] || fail "global mode is not remote"
m remote mode local --project alpha >/dev/null
grep -q 'mode = "local"' "$root/c/projects/alpha.toml" || fail "alpha's registration file was not written"
m task add "Chosen remote" --project alpha --body "x" --run-on remote >/dev/null
m task add "Follows alpha" --project alpha --body "y" >/dev/null
m task add "In beta" --project beta --body "z" >/dev/null

echo "== 1. projects"
[ "$(proj alpha mode)" = local ] || fail "alpha is not local"
[ "$(proj alpha source)" = project ] || fail "alpha's mode does not come from the project"
[ "$(proj beta mode)" = local ] || fail "beta is not local"
[ "$(proj beta source)" = "no [remote_runs]" ] || fail "beta's source is not its missing [remote_runs]"

echo "== 2. tasks"
[ "$(runon 0 mode)" = remote ] || fail "task 0 is not remote"
[ "$(runon 0 from)" = task ] || fail "task 0's mode does not come from the task"
[ "$(runon 1 mode)" = local ] || fail "alpha's other task is not local"
[ "$(runon 1 from)" = project ] || fail "task 1's mode does not come from the project"
[ "$(runon 2 where)" = local ] || fail "beta's task is not local"
m task show 0 | grep -q 'runs on: GitHub (task choice)' || fail "task show line"

echo "== 3. the old values"
m remote on >/dev/null
[ "$(m remote status --json | jf "j['mode']")" = remote ] || fail "remote on is not remote"
m remote off >/dev/null
[ "$(m remote status --json | jf "j['mode']")" = local ] || fail "remote off is not local"
m task run-on 1 github >/dev/null
[ "$(runon 1 mode)" = remote ] || fail "run-on github is not remote"
m task run-on 1 favor-remote >/dev/null
[ "$(runon 1 mode)" = favor-remote ] || fail "run-on favor-remote did not take"
m task run-on 1 default >/dev/null
[ "$(runon 1 from)" = project ] || fail "run-on default did not fall through to the project"

echo "== 4. refusals"
m remote mode sometimes --project beta >/dev/null 2>&1 && fail "a bad mode was accepted for a project"
m remote mode sometimes >/dev/null 2>&1 && fail "a bad mode was accepted"
echo "s816 passed"
