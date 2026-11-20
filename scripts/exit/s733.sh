#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s733 exit test: landing on GitHub (#702). A real masond with `mason remote on`, a project whose
# `[remote]` points at a fake GitHub on loopback, and a batch of two tasks, one of which holds a
# file `red.marker`: the fake answers its runs with a red artifact. The suite's own command
# always fails here, so a landed task proves its verdict came from GitHub. Checks:
#  1. the origin was sent pushes to `mason/run/**` for the batch's runs (landing asked);
#  2. the red task is sent back and the green one lands;
#  3. `mason landing` shows the batch's runs with where they ran and a run link.
set -euo pipefail
trap 'rc=$?; [ "$rc" -eq 0 ] || echo "FAILED (exit $rc) at line $LINENO: $BASH_COMMAND" >&2' ERR
cd "$(git rev-parse --show-toplevel)"
root=$(mktemp -d /tmp/mason-s733.XXXXXX); daemon=""; gh=""
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
fail() { echo "FAILED: $*" >&2; set +e; m landing >&2 2>&1; tail -30 "$root/masond.log" >&2; cat "$root/pushes.log" >&2 2>/dev/null; exit 1; }
wait_until() { # what seconds command...
  local what=$1 secs=$2; shift 2; local end=$((SECONDS + secs))
  until "$@"; do [ $SECONDS -lt $end ] || fail "$what"; sleep 0.2; done
}
state_is() { [ "$(m task show "$1" --json 2>/dev/null | jq_state)" = "$2" ]; }
n_landed() { local n=0 t; for t in 0 1; do state_is "$t" landed && n=$((n + 1)); done; echo "$n"; }
one_landed() { [ "$(n_landed)" = 1 ]; }
on_main() { git -C "$root/origin.git" cat-file -e "main:$1" 2>/dev/null; }

echo "== setup: a repository whose base branch has the workflow, a fake GitHub with two artifacts"
mkdir -p "$root/gh" "$root/c/projects"
echo '[]' > "$root/gh/issues.json"; echo '{}' > "$root/gh/deps.json"
git init -q --bare -b main "$root/origin.git"
cat > "$root/origin.git/hooks/pre-receive" <<HOOK
#!/bin/sh
while read old new ref; do echo "\$old \$new \$ref" >> "$root/pushes.log"; done
HOOK
chmod +x "$root/origin.git/hooks/pre-receive"
echo "$root/origin.git" > "$root/gh/origin-path"
python3 -I - "$root/gh" <<'PY'
import sys, zipfile
d = sys.argv[1]
for name, out, code in (
    ("run-artifact.zip", "1..3\nok 1 - one\nok 2 - two\nok 3 - three\n", 0),
    ("run-artifact-red.zip", "1..3\nok 1 - one\nok 2 - two\nnot ok 3 - three\n", 1),
):
    z = zipfile.ZipFile(f"{d}/{name}", "w")
    z.writestr("output.log", out)
    z.writestr("result.json", '{"code": %d, "signal": 0, "build_s": 1, "run_s": 2}' % code)
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

[step.develop]
type = "agent"
category = "developing"
next = "land"

[step.land]
type = "landing"
category = "landing"

[suite.unit]
command = "sh checks.sh"
parser = "tap"
min_tests = 3
level = "unit"
TOML
# Here the suite is always red: only a verdict from GitHub can land anything.
printf '#!/bin/sh\necho 1..3\necho "not ok 1 - here"\nexit 1\n' > "$root/repo/checks.sh"
printf 'window = "2s"\nmax_size = 5\n[[tier]]\nname = "batch"\nsuites = ["unit"]\n' > "$root/repo/.mason/landing.toml"
echo base > "$root/repo/base.txt"
git -C "$root/repo" add -A
git -C "$root/repo" -c user.name=t -c user.email=t@t commit -qm "base"
git -C "$root/repo" push -q origin main
git -C "$root/repo" fetch -q origin
: > "$root/pushes.log"

# The first agent to start makes the red tree; every agent commits one file of its own.
cat > "$root/script.json" <<JSON
{"steps": {"develop": [[{"sh": "if mkdir $root/first 2>/dev/null; then touch red.marker; fi; f=\$(mktemp ./f-XXXXXX) && echo x > \$f && git add -A && git -c user.name=t -c user.email=t@t commit -qm change"}, {"call": {"tool": "submit_change", "args": {"summary": "done", "head": "\$HEAD"}}}]]}}
JSON
cat > "$root/c/projects/alpha.toml" <<TOML
path = "$root/repo"
base = "main"
remote = "origin"
test = "true"
claude_bin = "$bin/fake-claude"
step_timeout_s = 300

[agent_env]
FAKE_CLAUDE_SCRIPT = "$root/script.json"

[remote]
kind = "github-actions"
api = "http://127.0.0.1:$port"
repo = "o/r"
token = "env:FAKE_GITHUB_TOKEN"
TOML
export FAKE_GITHUB_TOKEN=recorded-test-token
"$bin/masond" --port 0 --data-dir "$root/d" --config-dir "$root/c" --heartbeat-ms 100 --sweep-ms 3600000 --max-concurrent 4 --owner "$owner" 2>>"$root/masond.log" &
daemon=$!
until m status >/dev/null 2>&1; do kill -0 "$daemon" || { cat "$root/masond.log" >&2; exit 1; }; sleep 0.1; done
m remote on >/dev/null

echo "== a batch of two tasks, one red"
m task add "one" --project alpha --explored --body "x" >/dev/null
m task add "two" --project alpha --explored --body "x" >/dev/null
wait_until "exactly one task landed" 300 one_landed
! on_main red.marker || fail "the red task landed"

echo "== 1. the batch's runs went to GitHub"
runs=$(awk '$3 ~ /^refs\/heads\/mason\/run\// && $2 !~ /^0+$/' "$root/pushes.log" | wc -l | tr -d ' ')
[ "$runs" -ge 2 ] || fail "only $runs push(es) to mason/run/**"

echo "== 3. mason landing shows each run's place and its link"
m landing | grep -q 'ran on GitHub (ubuntu-latest), run http' || fail "mason landing shows no remote run link"
echo "s733 passed"
