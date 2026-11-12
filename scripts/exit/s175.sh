#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Scheduled CI runs (s175) exit test. A real masond against a fake GitHub. The project has a
# `kind = "ci"` schedule (off unless configured; nothing else turns it on).
#  1. `mason schedule run proj/nightly` decides (no green run yet) and dispatches the workflow.
#  2. `mason status` shows the run in flight.
#  3. The fake run ends with one job red twice and one red then green (and one green):
#     the failed jobs are rerun once, and exactly one red task and one flaky task are filed.
#  4. Running the schedule again does not dispatch (the tip was already tested) or file more.
# Nothing here needs the network: the fake GitHub listens on loopback.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
root=$(mktemp -d /tmp/mason-s175.XXXXXX); daemon=""; gh=""
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
fail() { echo "FAILED: $*" >&2; set +e; m task list >&2 2>&1; m schedule list >&2 2>&1; tail -20 "$root/masond.log" >&2; exit 1; }
count_of() { m task list | awk -v p="$1" '/^#/ && index($0, p) { n++ } END { print n + 0 }'; }
id_of() { m task list | grep "$1" | head -1 | sed -E 's/^#?([0-9]+).*/\1/'; }

echo "== setup: a repository, a fake GitHub, and a ci schedule in the project's overlay"
mkdir -p "$root/gh" "$root/c/projects"
echo '[]' > "$root/gh/issues.json"; echo '[]' > "$root/gh/wf-runs.json"
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
TOML
printf 'mode = "direct"\n' > "$root/repo/.mason/landing.toml"
echo base > "$root/repo/base.txt"
git -C "$root/repo" add -A
git -C "$root/repo" -c user.name=t -c user.email=t@t commit -qm "pipeline"
git -C "$root/repo" push -q origin main
tip=$(git -C "$root/repo" rev-parse HEAD)
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

[schedule.nightly]
kind = "ci"
workflow = "full.yml"
run_name = "Full"
inputs = { features = "" }
at = "23:00"
group_by = "<group>/<leg>"
max_tickets = 6
TOML
export FAKE_GITHUB_TOKEN=recorded-test-token
# The run the dispatch will create: it finishes red on its first attempt. Two jobs fail; on the
# rerun one is red again and the other is green.
cat > "$root/gh/dispatch-run.json" <<JSON
{"id": 77, "name": "CI", "head_sha": "$tip", "html_url": "https://github.com/o/r/actions/runs/77",
 "created_at": "2026-11-11T22:00:00Z", "status": "completed", "conclusion": "failure",
 "event": "workflow_dispatch", "display_title": "Full", "run_attempt": 1}
JSON
echo '[{"id": 771, "name": "alpha/linux", "conclusion": "failure"}, {"id": 772, "name": "beta/linux", "conclusion": "failure"}, {"id": 773, "name": "gamma/linux", "conclusion": "success"}]' > "$root/gh/jobs-77.json"
echo '[{"id": 781, "name": "alpha/linux", "conclusion": "failure"}, {"id": 782, "name": "beta/linux", "conclusion": "success"}, {"id": 783, "name": "gamma/linux", "conclusion": "success"}]' > "$root/gh/jobs-77-2.json"
printf 'step one\nFAILED alpha_scenario (first attempt)\n' > "$root/gh/log-771.txt"
printf 'step one\nFAILED beta_scenario (first attempt)\n' > "$root/gh/log-772.txt"
printf 'step one\nFAILED alpha_scenario (rerun)\n' > "$root/gh/log-781.txt"
# No task may start: the test is about what is filed.
"$bin/masond" --port 0 --data-dir "$root/d" --config-dir "$root/c" --heartbeat-ms 100 --sweep-ms 3600000 --schedule-ms 2000 --max-concurrent 0 --owner "$owner" 2>>"$root/masond.log" &
daemon=$!
until m status >/dev/null 2>&1; do kill -0 "$daemon" || { cat "$root/masond.log" >&2; exit 1; }; sleep 0.1; done

echo "== 1. mason schedule run dispatches (no green run yet)"
said=$(m schedule run proj/nightly)
echo "   $said"
case "$said" in dispatched\ full.yml\ on\ main*) ;; *) fail "not dispatched: $said" ;; esac
grep -q '"ref":"main"' "$root/gh/dispatch.log" || fail "the dispatch had no ref: $(cat "$root/gh/dispatch.log")"
grep -q '"features":""' "$root/gh/dispatch.log" || fail "the dispatch had no inputs"

echo "== 2. the run is shown in flight, and the schedule is listed"
m status | grep 'scheduled CI run in flight' | sed 's/^/   /' || fail "status does not show the followed run"
m schedule list | grep 'proj/nightly' | sed 's/^/   /'
m schedule list | grep -q ' ci ' || fail "the schedule list does not show the ci kind"

echo "== 3. the failed jobs are rerun once; one red task and one flaky task are filed"
end=$((SECONDS + 90))
until [ "$(count_of 'red twice')" = 1 ] && [ "$(count_of 'flaky')" = 1 ]; do
  [ $SECONDS -lt $end ] || fail "expected one red and one flaky task ($(count_of 'red twice') red, $(count_of 'flaky') flaky)"
  sleep 0.3
done
sleep 5
[ "$(count_of 'red twice')" = 1 ] && [ "$(count_of 'flaky')" = 1 ] || fail "more than one task of a kind"
[ "$(m task list | grep -c '^#')" = 2 ] || fail "other tasks were filed"
[ "$(wc -l < "$root/gh/rerun.log" | tr -d ' ')" = 1 ] || fail "rerun more than once: $(cat "$root/gh/rerun.log")"
red=$(id_of 'red twice'); flaky=$(id_of 'flaky')
m task show "$red" --json | grep -q 'CI/alpha/linux' || fail "red task #$red does not name alpha/linux"
m task show "$red" --json | grep -q 'alpha_scenario (rerun)' || fail "red task #$red has no log tail of the rerun"
m task show "$red" --json | grep -q '"priority": "normal"' || fail "red task #$red is not normal priority"
m task show "$flaky" --json | grep -q 'CI/beta/linux' || fail "flaky task #$flaky does not name beta/linux"
m task show "$flaky" --json | grep -q 'beta_scenario (first attempt)' || fail "flaky task #$flaky has no log tail of the failed attempt"
m task show "$flaky" --json | grep -q '"priority": "low"' || fail "flaky task #$flaky is not low priority"
m task list | grep '^#' | sed 's/^/   /'
if m status | grep -q 'scheduled CI run in flight'; then fail "the run is still shown in flight"; fi

echo "== 4. a second run does not dispatch again: the tip was already tested"
said=$(m schedule run proj/nightly)
echo "   $said"
case "$said" in not\ dispatched*) ;; *) fail "dispatched again: $said" ;; esac
[ "$(wc -l < "$root/gh/dispatch.log" | tr -d ' ')" = 1 ] || fail "dispatched twice"
[ "$(m task list | grep -c '^#')" = 2 ] || fail "a task was filed again"
echo "ok: dispatched, rerun once, one red task and one flaky task filed, no second dispatch"
