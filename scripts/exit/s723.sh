#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s723 exit test: a remote run leases GitHub runner pools, not this Mac's slots (#700). Same
# setup as s706 (a real masond, a fake GitHub on loopback, the suite forced remote by the
# test-only switch MASON_TEST_FORCE_REMOTE), but the project runs on a macOS runner and the
# fake keeps the run queued (`run-hold`) while the script looks at `mason resources --json`:
#  1. a holder (the task's test step) holds `gha` and `gha-macos` and none of `compile`,
#     `test-suite` or `test-small`;
#  2. `compile` and `test-suite` have no holder at all while the run is out;
#  3. when the fake lets the run complete the task lands and the pools have no holder.
set -euo pipefail
trap 'rc=$?; [ "$rc" -eq 0 ] || echo "FAILED (exit $rc) at line $LINENO: $BASH_COMMAND" >&2' ERR
cd "$(git rev-parse --show-toplevel)"
root=$(mktemp -d /tmp/mason-s723.XXXXXX); daemon=""; gh=""
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
fail() { echo "FAILED: $*" >&2; set +e; m task show 0 >&2 2>&1; echo "-- holders:" >&2; m resources --json >&2 2>&1; tail -30 "$root/masond.log" >&2; cat "$root/pushes.log" >&2 2>/dev/null; exit 1; }
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
runs_on = "macos-14"
TOML
export FAKE_GITHUB_TOKEN=recorded-test-token
MASON_TEST_FORCE_REMOTE=1 "$bin/masond" --port 0 --data-dir "$root/d" --config-dir "$root/c" --heartbeat-ms 100 --sweep-ms 3600000 --owner "$owner" 2>>"$root/masond.log" &
daemon=$!
until m status >/dev/null 2>&1; do kill -0 "$daemon" || { cat "$root/masond.log" >&2; exit 1; }; sleep 0.1; done

touch "$root/gh/run-hold"
echo "== a task runs its test step; the fake keeps the run queued"
m task add "Remote suite" --project alpha --body "x" > /dev/null
res() { m resources --json; }
end=$((SECONDS + 120))
until res | python3 -I -c '
import json, sys
v = json.load(sys.stdin)
sys.exit(0 if any("gha" in h["uses"] for h in v["holders"]) else 1)'; do
  [ $SECONDS -lt $end ] || fail "no holder of the gha pool appeared while the run was out"
  sleep 0.3
done
# The pool is leased before the run is dispatched and looked up, so wait for the lookup.
end=$((SECONDS + 120))
until grep -q 'head_sha=' "$root/gh/requests.log" 2>/dev/null; do
  [ $SECONDS -lt $end ] || fail "the run was not pushed and looked up"
  sleep 0.3
done

echo "== 1. the run holds the runner pools and none of this Mac's slots"
res > "$root/res.json"
python3 -I - "$root/res.json" <<'PY' || fail "the holders are not as expected"
import json, sys
v = json.load(open(sys.argv[1]))
caps = {r["name"]: r for r in v["resources"]}
assert "gha" in caps and "gha-macos" in caps, sorted(caps)
mine = [h for h in v["holders"] if "gha" in h["uses"]]
assert len(mine) == 1, v["holders"]
h = mine[0]
assert h["uses"].get("gha") == 1 and h["uses"].get("gha-macos") == 1, h
for m in ("compile", "test-suite", "test-small"):
    assert m not in h["uses"], h
assert not h["exclusive"], h
for other in v["holders"]:
    for m in ("compile", "test-suite", "test-small"):
        assert m not in other["uses"], other
PY

echo "== 2. the run completes and releases the pools"
rm -f "$root/gh/run-hold"
wait_state 0 landed 180
res | python3 -I -c '
import json, sys
v = json.load(sys.stdin)
assert not any("gha" in h["uses"] for h in v["holders"]), v["holders"]' || fail "a pool unit is still held after the run"
echo "s723 passed"
