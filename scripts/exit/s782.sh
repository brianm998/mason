#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# The full-test record and its triggers (s782, decision 0775).
#  1. The unit and integration tests: the quiet timer, the decision, the record and a restart.
#  2. A real masond against a fake GitHub, with a schedule that has `when_quiet` only:
#     `mason schedule list` shows "last full test: none on record" and the next trigger, and
#     `mason schedule run` starts one that the list then shows as running.
# Nothing here needs the network: the fake GitHub listens on loopback.
set -euo pipefail
unset MASON_EXIT_SHARD MASON_EXIT_PLAN MASON_EXIT_REPEAT MASON_EXIT_DEFAULT_WEIGHT MASON_EXIT_WEIGHTS
cd "$(git rev-parse --show-toplevel)"
root=$(mktemp -d /tmp/mason-s782.XXXXXX); daemon=""; gh=""
cleanup() {
  [ -n "$daemon" ] && kill -9 "$daemon" 2>/dev/null || true
  [ -n "$gh" ] && kill -9 "$gh" 2>/dev/null || true
  for f in "$root"/d/h/*/host.kv; do
    [ -e "$f" ] || continue
    for k in host_pid child_pid; do pid=$(sed -n "s/^$k=//p" "$f"); [ "${pid:-0}" -gt 1 ] 2>/dev/null || continue; kill -9 "-$pid" 2>/dev/null || kill -9 "$pid" 2>/dev/null || true; done
  done
  rm -rf "$root"
}
trap cleanup EXIT
echo "== 1. the tests of the rules, the record and the decision"
cargo test -q -p mason-core --lib -- full_test::
cargo test -q -p mason-store --test store -- full_test_rows
cargo test -q -p mason-daemon --test it -- ci_schedule::the_record ci_schedule::a_red_full ci_schedule::a_project_with_only
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

[schedule.full]
kind = "ci"
workflow = "full.yml"
run_name = "Full"
when_quiet = "30m"
TOML
export FAKE_GITHUB_TOKEN=recorded-test-token
# The run the dispatch will create stays in progress, so the full test is still running.
cat > "$root/gh/dispatch-run.json" <<JSON
{"id": 77, "name": "CI", "head_sha": "$tip", "html_url": "https://github.com/o/r/actions/runs/77",
 "created_at": "2026-11-11T22:00:00Z", "status": "in_progress", "conclusion": null,
 "event": "workflow_dispatch", "display_title": "Full", "run_attempt": 1}
JSON
# No task may start: the test is about what is filed.
"$bin/masond" --port 0 --data-dir "$root/d" --config-dir "$root/c" --heartbeat-ms 100 --sweep-ms 3600000 --schedule-ms 2000 --max-concurrent 0 --owner "$owner" 2>>"$root/masond.log" &
daemon=$!
until m status >/dev/null 2>&1; do kill -0 "$daemon" || { cat "$root/masond.log" >&2; exit 1; }; sleep 0.1; done

echo "== 2. mason schedule list shows the last full test line and the next trigger"
end=$((SECONDS + 30))
until m schedule list | grep -q 'proj/full'; do
  [ $SECONDS -lt $end ] || fail "the schedule is not listed"
  sleep 0.3
done
m schedule list | sed 's/^/   /'
m schedule list | grep -q 'last full test: none on record' || fail "no 'last full test' line"
m schedule list | grep -qE 'next: (when )?quiet for' || fail "no next trigger"

echo "== 3. a manual run starts one (same rule), and the list shows it running"
said=$(m schedule run proj/full)
echo "   $said"
case "$said" in dispatched\ full.yml\ on\ main*"(manual)"*) ;; *) fail "not dispatched: $said" ;; esac
m schedule list | grep -q 'running now: ' || fail "the list does not show the full test running"
m schedule list | grep 'last full test' | sed 's/^/   /'

echo "== 4. a second manual run does not start another: the running one is followed"
said=$(m schedule run proj/full)
echo "   $said"
case "$said" in dispatched*) fail "started a second: $said" ;; esac
[ "$(wc -l < "$root/gh/dispatch.log" | tr -d ' ')" = 1 ] || fail "dispatched twice"
echo "ok: the record shows, one rule decides, a running full test is not started twice"
