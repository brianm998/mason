#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# CI progress (s205) exit test. A real masond with a direct-ci project against a fake GitHub that
# serves a two-workflow CI still in progress: pr-gate has one job passed and one failed, macOS has
# one running and one queued. The task waits in landing; nothing ever finishes the CI.
#  1. The task's progress (HTTP API through `mason task progress --json`) has one sub per workflow
#     with the jobs told apart: summed, 4 jobs, 1 queued, 1 running (named), 1 passed, 1 failed.
#  2. The step message and the printed progress show them.
#  3. After several polls that change nothing the step is not shown as "no output for".
# Nothing here needs the network: the fake GitHub listens on loopback.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
root=$(mktemp -d /tmp/mason-s205.XXXXXX); daemon=""; gh=""
cleanup() {
  [ -n "$daemon" ] && kill -9 "$daemon" 2>/dev/null || true
  [ -n "$gh" ] && kill -9 "$gh" 2>/dev/null || true
  for f in "$root"/d/h/*/host.kv; do
    [ -e "$f" ] || continue
    for k in host_pid child_pid; do pid=$(sed -n "s/^$k=//p" "$f"); kill -9 "-$pid" 2>/dev/null || kill -9 "$pid" 2>/dev/null || true; done
  done
  rm -rf "$root"
}
trap cleanup EXIT
cargo build -q --workspace --bins
bin=target/debug
owner=${USER:-owner}
m() { "$bin/mason" --data-dir "$root/d" --config-dir "$root/c" "$@"; }
fail() { echo "FAILED: $*" >&2; set +e; m task list >&2 2>&1; m task progress "${t:-1}" >&2 2>&1; tail -20 "$root/masond.log" >&2; exit 1; }

echo "== setup: a repository, a direct-ci project, and a fake GitHub with an unfinished CI"
mkdir -p "$root/gh" "$root/c/projects"
echo '[]' > "$root/gh/issues.json"
cat > "$root/gh/runs.json" <<'JSON'
[{"id": 101, "name": "pr-gate", "status": "in_progress", "conclusion": null},
 {"id": 102, "name": "macOS", "status": "in_progress", "conclusion": null}]
JSON
echo '[{"id": 1011, "name": "lint", "status": "completed", "conclusion": "success"}, {"id": 1012, "name": "unit", "status": "completed", "conclusion": "failure"}]' > "$root/gh/jobs-101.json"
echo '[{"id": 1021, "name": "macOS build", "status": "in_progress", "conclusion": null}, {"id": 1022, "name": "macOS ui", "status": "queued", "conclusion": null}]' > "$root/gh/jobs-102.json"
"$bin/fake-github" "$root/gh" "$root/gh.port" 2>"$root/gh.log" & gh=$!
until [ -s "$root/gh.port" ]; do sleep 0.05; done
port=$(cat "$root/gh.port")
git init -q --bare -b main "$root/origin.git"
git clone -q "$root/origin.git" "$root/repo" 2>/dev/null
git -C "$root/repo" checkout -q -b main
mkdir -p "$root/repo/.mason"
cat > "$root/repo/.mason/mason.toml" <<'TOML'
schema = 1
start = "develop"

[step.develop]
type = "command"
category = "developing"
command = "echo change > change.txt && git add change.txt && git -c user.name=t -c user.email=t@t commit -qm change"
next = "test"

[step.test]
type = "test"
category = "testing"
max_rounds = 1
on_red = "develop"
next = "land"

[step.land]
type = "landing"
category = "landing"
TOML
printf 'mode = "direct-ci"\n' > "$root/repo/.mason/landing.toml"
echo base > "$root/repo/base.txt"
git -C "$root/repo" add -A
git -C "$root/repo" -c user.name=t -c user.email=t@t commit -qm "pipeline"
git -C "$root/repo" push -q origin main
cat > "$root/c/projects/proj.toml" <<TOML
path = "$root/repo"
base = "main"
remote = "origin"
test = "true"

[ci]
kind = "github-actions"
api = "http://127.0.0.1:$port"
repo = "o/r"
token = "env:FAKE_GITHUB_TOKEN"
poll_s = 1
TOML
export FAKE_GITHUB_TOKEN=recorded-test-token
"$bin/masond" --port 0 --data-dir "$root/d" --config-dir "$root/c" --heartbeat-ms 100 --sweep-ms 3600000 --owner "$owner" 2>>"$root/masond.log" &
daemon=$!
until m status >/dev/null 2>&1; do kill -0 "$daemon" || { cat "$root/masond.log" >&2; exit 1; }; sleep 0.1; done
t=$(m task add "change" --project proj --body "add a file" | sed -n 's/^Filed task #\([0-9]*\) .*/\1/p')

field() {  # PYTHON-EXPRESSION over the first record `p` of the task's progress
  m task progress "$t" --json | python3 -I -c "import json,sys; d=json.load(sys.stdin); p=d['runs'][0]; print($1)"
}

echo "== 1. the CI's jobs are told apart, per workflow and summed"
end=$((SECONDS + 120))
until [ "$(m task progress "$t" --json 2>/dev/null | python3 -I -c "
import json,sys
d=json.load(sys.stdin)
print(sum(1 for r in d['runs'] for s in r['subs'] if s.get('jobs')))" 2>/dev/null || echo 0)" -ge 2 ]; do
  [ $SECONDS -lt $end ] || fail "no per-workflow job counts appeared"
  sleep 0.5
done
sum=$(field "[sum(s['jobs'][k] for s in p['subs'] if s.get('jobs')) for k in ('queued','running','succeeded','failed')]")
[ "$sum" = "[1, 1, 1, 1]" ] || fail "queued/running/succeeded/failed were $sum"
total=$(field "sum(s['total']['value'] for s in p['subs'] if s.get('jobs'))")
[ "$total" = 4 ] || fail "total jobs were $total"
names=$(field "[n for s in p['subs'] if s.get('jobs') for n in s['jobs']['running_names']]")
[ "$names" = "['macOS build']" ] || fail "running names were $names"
subnames=$(field "sorted(s['name'] for s in p['subs'] if s.get('jobs'))")
[ "$subnames" = "['macOS', 'pr-gate']" ] || fail "one sub per workflow expected, got $subnames"

echo "== 2. the step message and the printed progress say it"
msg=$(field "p['message']")
echo "   $msg"
case "$msg" in *"4 jobs: 1 queued, 1 running (macOS build), 1 passed, 1 failed"*) ;; *) fail "message: $msg" ;; esac
m task progress "$t" | grep -q 'macOS: 2 jobs · 1 queued · 1 running (macOS build)' || fail "no workflow line in: $(m task progress "$t")"
m task progress "$t" | grep -q 'pr-gate: 2 jobs · 1 passed · 1 failed' || fail "no pr-gate line in: $(m task progress "$t")"

echo "== 3. after several unchanged polls the step is not quiet"
polls() { grep -c 'actions/runs?head_sha' "$root/gh/requests.log"; }
before=$(polls)
end=$((SECONDS + 60))
until [ "$(( $(polls) - before ))" -ge 6 ]; do
  [ $SECONDS -lt $end ] || fail "the CI was not polled again"
  sleep 0.5
done
if m task progress "$t" | grep -q 'no output for'; then fail "a working CI is shown as quiet: $(m task progress "$t")"; fi
echo "ok: 4 jobs told apart per workflow (1 queued, 1 running, 1 passed, 1 failed), and never 'no output' while polls are answered"
