#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# The base-branch CI watch (s154) exit test. A real masond against a fake GitHub that serves the
# base branch's workflow runs. The project's definition turns `[watch.ci]` on, with a gate job.
#  1. A green run is served: the watch seeds, and files nothing.
#  2. A gate run goes red: exactly one urgent task is filed, and landing is frozen.
#  3. More red runs: still one task, still frozen.
#  4. A green run is served: the freeze is lifted.
# Nothing here needs the network: the fake GitHub listens on loopback.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
root=$(mktemp -d /tmp/mason-s154.XXXXXX); daemon=""; gh=""
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
fail() { echo "FAILED: $*" >&2; set +e; m task list >&2 2>&1; m status >&2 2>&1; tail -20 "$root/masond.log" >&2; exit 1; }
count_of() { m task list | awk -v p="$1" '/^#/ && index($0, p) { n++ } END { print n + 0 }'; }
id_of() { m task list | grep "$1" | head -1 | sed -E 's/^#?([0-9]+).*/\1/'; }
frozen() { m landing | awk '/^FROZEN/ { n++ } END { print n + 0 }'; }
RUNS=""
run() {  # ID CONCLUSION GATE-JOB-CONCLUSION: serve one more finished run on the base branch
  local id=$1 when
  when=$(printf '2026-11-11T10:%02d:00Z' "$id")
  RUNS=$(printf '{"id": %s, "name": "CI", "head_sha": "%040d", "html_url": "https://github.com/o/r/actions/runs/%s", "created_at": "%s", "status": "completed", "conclusion": "%s"}%s%s' \
    "$id" "$id" "$id" "$when" "$2" "${RUNS:+,}" "$RUNS")
  printf '[%s]\n' "$RUNS" > "$root/gh/runs.json.tmp"; mv "$root/gh/runs.json.tmp" "$root/gh/runs.json"
  printf '[{"id": %s1, "name": "pr-gate", "conclusion": "%s"}, {"id": %s2, "name": "lint", "conclusion": "success"}]\n' "$id" "$3" "$id" > "$root/gh/jobs-$id.json"
}

echo "== setup: a repository with [watch.ci] on, and a fake GitHub serving base-branch runs"
mkdir -p "$root/gh" "$root/c/projects"
echo '[]' > "$root/gh/issues.json"; echo '[]' > "$root/gh/runs.json"
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
command = "true"
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

[watch.ci]
enabled = true
gate = "CI/pr-gate"
exclude = ["Gherkin"]
TOML
printf 'mode = "direct"\n' > "$root/repo/.mason/landing.toml"
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
run 1 success success
# No task may start: the test is about what is filed and frozen.
"$bin/masond" --port 0 --data-dir "$root/d" --config-dir "$root/c" --heartbeat-ms 100 --sweep-ms 3600000 --max-concurrent 0 --owner "$owner" 2>>"$root/masond.log" &
daemon=$!
until m status >/dev/null 2>&1; do kill -0 "$daemon" || { cat "$root/masond.log" >&2; exit 1; }; sleep 0.1; done

echo "== 1. a green run: the watch seeds and files nothing"
end=$((SECONDS + 60))
until m status | grep -q 'watching CI'; do
  [ $SECONDS -lt $end ] || fail "the watch never saw the workflow"
  sleep 0.3
done
[ "$(count_of 'red on')" = 0 ] || fail "seeding filed a task"
[ "$(frozen)" = 0 ] || fail "seeding froze landing"

echo "== 2. the gate goes red: one urgent task, landing frozen"
printf 'step one\nstep two\nFAILED test_gate\n' > "$root/gh/log-21.txt"
run 2 failure failure
end=$((SECONDS + 60))
until [ "$(count_of 'CI/pr-gate red on main')" = 1 ] && [ "$(frozen)" = 1 ]; do
  [ $SECONDS -lt $end ] || fail "no task and freeze for the red gate ($(count_of 'CI/pr-gate red on main') tasks, $(frozen) freezes)"
  sleep 0.3
done
id=$(id_of 'CI/pr-gate red on main')
m task show "$id" --json | grep -q '"priority": "urgent"' || fail "task #$id is not urgent"
m task show "$id" --json | grep -q 'FAILED test_gate' || fail "task #$id has no log tail"
m task show "$id" --json | grep -q 'actions/runs/2' || fail "task #$id has no run link"
m landing | grep '^FROZEN' | sed 's/^/   /'
m status | grep -E 'ci watch|LANDING FROZEN|filed:' | sed 's/^/   /'

echo "== 3. more red runs: still one task, still frozen"
run 3 failure failure
sleep 4
[ "$(count_of 'CI/pr-gate')" = 1 ] || fail "a second task was filed: $(count_of 'CI/pr-gate')"
[ "$(frozen)" = 1 ] || fail "the freeze lifted while red"

echo "== 4. green on the base: the freeze lifts"
run 4 success success
end=$((SECONDS + 60))
until [ "$(frozen)" = 0 ]; do
  [ $SECONDS -lt $end ] || fail "the freeze never lifted"
  sleep 0.3
done
[ "$(count_of 'CI/pr-gate')" = 1 ] || fail "green filed a task"
echo "ok: seeded, one urgent task and a freeze for the red gate, lifted by a green run"
