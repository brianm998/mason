#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1093 exit test: the nightly pipeline review.
#  1. The tests of its rules, by name: settings, when it is due, one review for every included
#     project, the six questions, opus at medium, the duplicate search over every task, findings
#     proposed and capped, and the result file.
#  2. A real masond, a fake agent (fake-claude) and a local git repository; no real service, no
#     real agent CLI, no network. `mason review status` shows enabled at 00:00 every day; a change
#     is read back; `mason review run` files one task in MASON's own project with the six
#     questions on opus at medium; the agent writes three findings, one about a task that already
#     landed: two are filed as proposed and one is dropped, naming that task.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

run() {
  local what=$1; shift
  echo "== $what"
  # Where `cargo test --workspace` has just run, it ran every one of these.
  [ -z "${MASON_TESTS_ALREADY_RAN:-}" ] || return 0
  out=$(cargo test -q "$@" 2>&1) || { echo "$out" >&2; echo "FAILED: $what" >&2; exit 1; }
  echo "$out" | grep -E '^test result' | tail -3
  echo "$out" | grep -qE 'test result: ok\. [1-9]' || { echo "FAILED: $what ran no test" >&2; exit 1; }
}

run "due at the local midnight, once after a gap, weekly" -p mason-core --lib -- a_nightly_job_is_due
run "findings name a category; a detector still needs a metric" -p mason-core --lib -- \
  a_review_finding_names_a_category a_detector_finding_still_needs a_draft_gives_severity \
  the_analyst_is_held
run "the review's notice is routed" -p mason-core --lib -- every_raised_kind_is_routed
run "the settings file, the notice, the result file" -p mason-daemon --lib -- \
  review::tests the_review_file_is_read a_duplicate_refusal_names
run "the review in a daemon" -p mason-daemon --test it -- review:: introspection_policy::

echo "== a real daemon"
root=$(mktemp -d /tmp/mason-s1093.XXXXXX); daemon=""
cleanup() {
  [ -n "$daemon" ] && kill -9 "$daemon" 2>/dev/null || true
  for f in "$root"/d/h/*/host.kv; do
    [ -e "$f" ] || continue
    hp=$(sed -n "s/^host_pid=//p" "$f"); cp=$(sed -n "s/^child_pid=//p" "$f"); if [ -n "$hp" ] && ps -o command= -p "$hp" 2>/dev/null | grep -qF "step-host --dir $(dirname "$f")"; then kill -9 "-$cp" 2>/dev/null || true; kill -9 "$hp" 2>/dev/null || true; fi  # only a live step host: a stale pid may belong to someone else now
  done
  rm -rf "$root"
}
trap cleanup EXIT
cargo build -q --workspace --bins
bin=target/debug
owner=${USER:-owner}
export MASON_AS="$owner"
m() { "$bin/mason" --data-dir "$root/d" --config-dir "$root/c" "$@"; }
jq_state() { sed -n 's/^  "state": "\(.*\)",$/\1/p'; }
wait_state() {  # TASK STATE SECONDS
  local end=$((SECONDS + $3))
  until [ "$(m task show "$1" --json 2>/dev/null | jq_state)" = "$2" ]; do
    [ $SECONDS -lt $end ] || { echo "timed out waiting for task $1 to be $2" >&2; m task show "$1" >&2 || true; tail -30 "$root/masond.log" >&2; exit 1; }
    sleep 0.3
  done
}

git init -q --bare -b main "$root/origin.git"
git clone -q "$root/origin.git" "$root/mason" 2>/dev/null
git -C "$root/mason" checkout -q -b main
mkdir -p "$root/mason/.mason" "$root/mason/crates/daemon/src" "$root/mason/docs"
cat > "$root/mason/.mason/mason.toml" <<'TOML'
schema = 1
start = "develop"

[step.develop]
type = "agent"
category = "developing"
next = "test"

[step.test]
type = "test"
category = "testing"
max_rounds = 2
on_red = "develop"
next = "land"

[step.land]
type = "landing"
category = "landing"
TOML
printf 'mode = "direct"\n' > "$root/mason/.mason/landing.toml"
echo "// ranks waiting steps" > "$root/mason/crates/daemon/src/sched.rs"
echo "notes" > "$root/mason/docs/notes.md"
git -C "$root/mason" add -A
git -C "$root/mason" -c user.name=t -c user.email=t@t commit -qm start
git -C "$root/mason" push -q origin main
git -C "$root/mason" checkout -q -b owner-work

cat > "$root/agent.json" <<'JSON'
{"steps": {"develop": [[
  {"sh": "echo fix >> docs/notes.md && git add -A && git -c user.name=t -c user.email=t@t commit -qm fix"},
  {"call": {"tool": "submit_change", "args": {"summary": "fix", "head": "$HEAD"}}}
]]}}
JSON
mkdir -p "$root/c/projects"
cat > "$root/c/projects/mason.toml" <<TOML
path = "$root/mason"
base = "main"
remote = "origin"
test = "true"
claude_bin = "$PWD/$bin/fake-claude"
[agent_env]
FAKE_CLAUDE_SCRIPT = "$root/agent.json"

[self]
max_open = 20
per_week = 50
max_steps = 500
TOML
"$bin/masond" --port 0 --data-dir "$root/d" --config-dir "$root/c" --heartbeat-ms 100 --sweep-ms 3600000 \
  --schedule-ms 300 --owner "$owner" 2>>"$root/masond.log" &
daemon=$!
until m status >/dev/null 2>&1; do kill -0 "$daemon" || { cat "$root/masond.log" >&2; exit 1; }; sleep 0.1; done

echo "== the defaults, then a change read back"
out=$(m review status); echo "$out" | sed 's/^/   /'
grep -q "Pipeline review: enabled at 00:00 every 1 day (opus, medium; at most 10 proposals)" <<<"$out"
m review set --at 03:30 --every-days 7 >/dev/null
out=$(m review status); echo "$out" | sed 's/^/   /'
grep -q "enabled at 03:30 every 7 days" <<<"$out"
grep -q '^at = "03:30"' "$root/c/review.toml"
grep -q '^changed_by = ' "$root/c/review.toml"
if m review set --every-days 31 2>/dev/null; then echo "31 days was accepted" >&2; exit 1; fi

echo "== a task lands, the one a later finding repeats"
m task add "Landing queue idles between batches" --project mason --explored --body "The landing queue idles between batches." >/dev/null
landed=$(m task list | sed -n 's/^#\([0-9]*\) .*Landing queue idles between batches.*/\1/p' | head -1)
wait_state "$landed" landed 120
echo "   task #$landed landed"

cat > "$root/result.json" <<'JSON'
{"findings": [
  {"category": "speed", "title": "Landing queue idles between batches", "body": "The landing queue idles between batches.", "evidence": "crates/daemon/src/sched.rs:1 shows the queue waiting"},
  {"category": "coverage", "title": "No test covers a takeover", "body": "A takeover is never exercised.", "evidence": "crates/daemon/src/sched.rs:1 has no test beside it"},
  {"category": "automate", "title": "Owner retries outside failures by hand", "body": "The owner retries what the daemon could.", "evidence": "docs/notes.md:1 records the retries"}
],
"nothing_found": [
  {"category": "stuck", "because": "nothing waited long"},
  {"category": "narrow", "because": "every test run was needed"},
  {"category": "crash", "because": "no job panicked"}
]}
JSON
cat > "$root/agent.json" <<JSON
{"steps": {"develop": [[
  {"sh": "test -f review-facts/tasks.json && cp $root/result.json analyst-result.json"},
  {"call": {"tool": "submit_change", "args": {"summary": "reviewed; dropped one duplicate of #$landed", "head": "\$HEAD"}}}
]]}}
JSON

echo "== run it now"
said=$(m review run); echo "   $said"
review=$(sed -n 's/^review task #\([0-9]*\) filed$/\1/p' <<<"$said")
[ -n "$review" ]
wait_state "$review" landed 120
shown=$(m task show "$review" --json)
[ "$(m task list | grep -c 'Analyse recent pipeline behaviour')" = 1 ]
m task show "$review" --json | grep -q '"project": "mason"'
m task show "$review" --json | grep -q '"priority": "low"'
for q in "Where was the pipeline stuck?" "Where could the pipeline be faster?" \
  "test more than it needed to at an early stage" "Where is there a gap in test coverage?" \
  "Where did the pipeline crash, in any way?" "automate more and involve the owner less?"; do
  grep -qF "$q" <<<"$shown" || { echo "the task does not ask: $q" >&2; echo "$shown" >&2; exit 1; }
done
m task show "$review" --json | python3 -c "
import json, sys
first = [l for s in json.load(sys.stdin)['steps'] for l in s['launches']][0]
assert (first['requested_model'], first['effort'], first['overridden']) == ('opus', 'medium', True), first
print('   launched:', first['display'])
"
echo "   one low task in MASON's own project, six questions, opus at medium"

echo "== what it filed"
m proposals | sed 's/^/   /'
[ "$(m proposals --json | grep -c '"task":')" = 2 ]
[ "$(m proposals --json | grep -c '"state": "proposed"')" = 2 ]
m review status --json | python3 -c "
import json, sys
r = json.load(sys.stdin)['last']
assert r['task'] == $review, r
assert len(r['filed']) == 2, r
assert [(d['task'], d['by']) for d in r['duplicates']] == [($landed, 'mason')], r
assert len(r['nothing_found']) == 3, r
"
out=$(m review status); echo "$out" | sed 's/^/   /'
grep -q "2 filed" <<<"$out"
grep -qF "\"Landing queue idles between batches\" is #$landed" <<<"$out"
out=$(m schedule list); grep -q '^review ' <<<"$out"
if grep -q 'analyst:' <<<"$out"; then echo "an analyst row is still listed" >&2; exit 1; fi
echo "   two proposals wait for the owner; the third was dropped as a duplicate of #$landed"

trap - EXIT; cleanup
echo "s1093 OK"
