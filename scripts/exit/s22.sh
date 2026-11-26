#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Stage 22 exit test: introspection and recurring jobs. A real masond, fake agents (fake-claude)
# and local git repositories; no real service, no real agent CLI, no network.
#  1. A retry loop on a permanent error is planted in recorded history (the step_attempts table,
#     through the plant example). The daily detector finds it and files a proposal with evidence
#     and the metric it expects to move; a transient error planted beside it is not a finding.
#     The owner approves, a scripted fix lands on MASON's repository, history after it has no
#     retries, and the digest says "this fix worked" with the before and after values.
#  2. A second fix that changes nothing is reported as "this fix didn't help".
#  3. The analyst's findings are checked like any proposal: one filed, two refused.
#  4. Triage (off by default) records what it would do and queues it for review; it answers nothing.
#  5. The digest shows exploring skips by route and forced landings with their reasons.
#  6. Base health files only failures that the baseline did not list.
#  7. A schedule runs by itself and does not pile up tasks.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
root=$(mktemp -d /tmp/mason-s22.XXXXXX); daemon=""
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
cargo build -q -p mason-store --example plant
bin=target/debug
plant="$PWD/$bin/examples/plant"
owner=${USER:-owner}
m() { "$bin/mason" --data-dir "$root/d" --config-dir "$root/c" "$@"; }
jq_state() { sed -n 's/^  "state": "\(.*\)",$/\1/p'; }
wait_state() {  # TASK STATE SECONDS
  local end=$((SECONDS + $3))
  until [ "$(m task show "$1" --json 2>/dev/null | jq_state)" = "$2" ]; do
    [ $SECONDS -lt $end ] || { echo "timed out waiting for task $1 to be $2" >&2; m task show "$1" >&2 || true; tail -20 "$root/masond.log" >&2; exit 1; }
    sleep 0.3
  done
}
wait_for() {  # SECONDS WHAT CMD...: until the command succeeds
  local end=$((SECONDS + $1)) what=$2; shift 2
  until "$@" >/dev/null 2>&1; do
    [ $SECONDS -lt $end ] || { echo "timed out waiting for $what" >&2; m health >&2 || true; tail -30 "$root/masond.log" >&2; exit 1; }
    sleep 0.3
  done
}
commit='git add -A && git -c user.name=t -c user.email=t@t commit -qm'
def='schema = 1
start = "explore"
explored_entry = "develop"

[step.explore]
type = "agent"
category = "exploring"
next = "develop"

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
'
repo_of() {  # NAME: a bare "origin" and a clone with the pipeline's definition on main
  git init -q --bare -b main "$root/$1-origin.git"
  git clone -q "$root/$1-origin.git" "$root/$1" 2>/dev/null
  git -C "$root/$1" checkout -q -b main
  mkdir -p "$root/$1/.mason"
  printf '%s' "$def" > "$root/$1/.mason/mason.toml"
  printf 'mode = "direct"\n' > "$root/$1/.mason/landing.toml"
}
push_first() {  # NAME
  git -C "$root/$1" add -A; git -C "$root/$1" -c user.name=t -c user.email=t@t commit -qm "$1"
  git -C "$root/$1" push -q origin main
  git -C "$root/$1" checkout -q -b owner-work
}
turn='{"sh": "echo fix > docs/fix-$(date +%s).md; '"$commit"' fix"}, {"call": {"tool": "submit_change", "args": {"summary": "fix", "head": "$HEAD"}}}'

# --- MASON's own repository ("mason"): where a pipeline fix lands. It has the files the
# detectors point at, so a finding that cites them is routed here.
repo_of mason
mkdir -p "$root/mason/crates/daemon/src" "$root/mason/docs"
echo "// retries a failed step" > "$root/mason/crates/daemon/src/jobs.rs"
echo "// ranks waiting steps" > "$root/mason/crates/daemon/src/sched.rs"
echo "notes" > "$root/mason/docs/notes.md"
push_first mason
# --- a client project ("acmeshop"): the history is planted for it, and it has a base-health test
repo_of acmeshop
cat > "$root/acmeshop/tests.sh" <<'SH'
echo "1..2"; echo "ok 1 - checkout"; echo "not ok 2 - old_failure"
SH
push_first acmeshop
# --- "watchshop": its agent plays the analyst; "askshop": its agent asks a question
repo_of watchshop; echo w > "$root/watchshop/w.txt"; push_first watchshop
repo_of askshop; echo a > "$root/askshop/a.txt"; push_first askshop

cat > "$root/mason.json" <<JSON
{"steps": {"develop": [[$turn]]}}
JSON
cat > "$root/acme.json" <<JSON
{"steps": {"develop": [[
  {"sh": "echo done > done-\$(date +%s%N).txt; $commit work"}, {"call": {"tool": "submit_change", "args": {"summary": "work", "head": "\$HEAD"}}}
]]}}
JSON
# The analyst writes three findings: one good, one with no evidence, one naming a metric that does not exist.
cat > "$root/analyst-result.json" <<'JSON'
{"findings": [
  {"title": "Review steps queue behind develop steps", "body": "Review waits for a slot while develop holds them.", "evidence": "crates/daemon/src/sched.rs:1 ranks review below develop", "metric": "wait_share"},
  {"title": "Something feels slow", "body": "It just feels slow.", "evidence": "it feels slow to me", "metric": "wait_share"},
  {"title": "Morale is low in the logs", "body": "The logs look sad.", "evidence": "crates/daemon/src/jobs.rs:1 is gloomy", "metric": "happiness"}
]}
JSON
cat > "$root/watch.json" <<JSON
{"steps": {"develop": [[
  {"sh": "cp $root/analyst-result.json analyst-result.json"}, {"call": {"tool": "submit_change", "args": {"summary": "analysed", "head": "\$HEAD"}}}
]]}}
JSON
cat > "$root/ask.json" <<JSON
{"steps": {"develop": [[
  {"call": {"tool": "ask", "args": {"ask": "Should the token expire after one hour or one day?", "options": [{"label": "one hour", "recommended": true}, {"label": "one day"}]}}}
]]}}
JSON
cat > "$root/triage.sh" <<'SH'
cat > /dev/null
echo '{"action": "answer", "answer": "one hour", "evidence": "README.md:1 says one hour"}'
SH

mkdir -p "$root/c/projects"
proj() {  # NAME SCRIPT EXTRA
  cat > "$root/c/projects/$1.toml" <<TOML
path = "$root/$1"
base = "main"
remote = "origin"
test = "true"
claude_bin = "$PWD/$bin/fake-claude"
[agent_env]
FAKE_CLAUDE_SCRIPT = "$root/$2"
$3
TOML
}
proj mason mason.json '
[self]
max_open = 20
per_week = 50
max_steps = 500
'
proj acmeshop acme.json '
[schedule.base]
kind = "base-health"
every = "1h"
command = "sh tests.sh"
parser = "tap"
'
proj watchshop watch.json ''
proj askshop ask.json '
[triage]
enabled = true
sample_percent = 100
command = "sh '"$root"'/triage.sh"
'
start_daemon() {
  "$bin/masond" --data-dir "$root/d" --config-dir "$root/c" --heartbeat-ms 100 --sweep-ms 3600000 \
    --introspect-ms 1500 --schedule-ms 300 --analyst-ms 36000000 --owner "$owner" 2>>"$root/masond.log" &
  daemon=$!
  until m status >/dev/null 2>&1; do kill -0 "$daemon" || { cat "$root/masond.log" >&2; exit 1; }; sleep 0.1; done
}
start_daemon
db="$root/d/store.db"
# Only the project being tested runs: the others wait until their part.
m pause --mode claims --for 2h >/dev/null
count_proposals() { m proposals --json | awk '/"task"/ {n++} END {print n + 0}'; }
finding_field() {  # ID FIELD: from `mason health --json` (one finding per block)
  m health --json | python3 -c "import json,sys; f=[x for x in json.load(sys.stdin)['findings'] if x['id']==$1][0]; print($2)"
}

echo "== 1. a retry loop on a permanent error, planted in recorded history"
# five attempts of one step of one task, each failing for a reason no retry can change
"$plant" attempts "$db" --project acmeshop --task 77 --step develop --outcome fail \
  --reason "the agent failed: unknown model fable-9 (request 41)" --count 5 --retry-from 1
# and, beside it, two timeouts: transient, so no loop
"$plant" attempts "$db" --project acmeshop --task 78 --step develop --outcome fail \
  --reason "the step ran out of time" --count 2 --retry-from 2
wait_for 30 "the detector's proposal" sh -c "$bin/mason --data-dir $root/d --config-dir $root/c proposals | grep -q 'Retry loop on a permanent error'"
m proposals | sed 's/^/   /'
[ "$(count_proposals)" = 1 ]
prop=$(m proposals --json | sed -n 's/^ *"task": \([0-9]*\),$/\1/p' | head -1)
m proposal show "$prop" | sed 's/^/   | /' | head -12
m proposal show "$prop" --json | grep -q '"route": "pipeline"'   # cites MASON's own jobs.rs
m proposal show "$prop" --json | grep -q 'step_attempts'          # evidence from recorded rows
m proposal show "$prop" | grep -q 'retry_loop_rate'              # the metric it expects to move
m proposal show "$prop" | grep -q 'crates/daemon/src/jobs.rs'
echo "   filed as proposal #$prop, routed to MASON, naming retry_loop_rate; the two timeouts filed nothing"
m health | sed 's/^/   /'
[ "$(finding_field 1 "f['verdict']")" = open ]
[ "$(finding_field 1 "f['metric']")" = retry_loop_rate ]
[ "$(finding_field 1 "f['before']['num']")" = 4.0 ]
[ "$(finding_field 1 "f['before']['den']")" = 7.0 ]

echo "== 1. it is not filed again by later cycles"
sleep 5
[ "$(count_proposals)" = 1 ]
echo "   three more cycles, still one proposal"

echo "== 1. the owner approves; the scripted fix lands on MASON's repository"
m proposal approve "$prop"
m resume >/dev/null
wait_state "$prop" landed ${LAND_WAIT:-120}
git -C "$root/mason-origin.git" ls-tree -r --name-only main | grep -q '^docs/fix-'
m pause --mode claims --for 2h >/dev/null
echo "   task #$prop landed; MASON's main has the fix"
# Before any history after the fix there is no verdict: it waits for enough samples.
[ "$(finding_field 1 "f['verdict']")" = open ]
echo "   no verdict yet (nothing recorded since the fix landed)"

echo "== 1. history after the fix has no retries: the next cycle says it worked"
"$plant" attempts "$db" --project acmeshop --task 80 --step develop --outcome green --count 8 --retry-from 1
wait_for 30 "the verdict" sh -c "$bin/mason --data-dir $root/d --config-dir $root/c health | grep -q 'this fix worked'"
m health | grep -A1 'fix worked' | sed 's/^/   /' | head -3
[ "$(finding_field 1 "f['verdict']")" = worked ]
[ "$(finding_field 1 "round(f['after']['rate'], 3)")" = 0.0 ]
[ "$(finding_field 1 "round(f['before']['rate'], 3)")" = 0.571 ]
m digest --days 1 | grep -q "this fix worked: retry_loop_rate went from 0.571 to 0.000"
echo "   digest: $(m digest --days 1 | grep 'this fix worked' | sed 's/^ *//')"
sleep 4
[ "$(count_proposals)" = 1 ]
echo "   and the old loop is not filed again: the rows before the fix no longer count"

echo "== 2. a second problem, and a fix that changes nothing"
"$plant" attempts "$db" --project acmeshop --task 81 --step test --outcome fail \
  --reason "permission denied reading /srv/build/cache" --count 5 --retry-from 1
wait_for 30 "the second proposal" sh -c "[ \$($bin/mason --data-dir $root/d --config-dir $root/c proposals --json | grep -c '\"task\"') = 2 ]"
prop2=$(m proposals --json | sed -n 's/^ *"task": \([0-9]*\),$/\1/p' | head -1)
[ "$prop2" != "$prop" ]
m proposal show "$prop2" | grep -q 'Retry loop on a permanent error'
echo "   proposal #$prop2 filed for the new signature"
# The fix does nothing for the metric: a change to a note, nothing about retries.
cat > "$root/mason.json" <<JSON
{"steps": {"develop": [[{"sh": "echo note >> docs/notes.md; $commit note"}, {"call": {"tool": "submit_change", "args": {"summary": "note", "head": "\$HEAD"}}}]]}}
JSON
m proposal approve "$prop2"
m resume >/dev/null
wait_state "$prop2" landed ${LAND_WAIT:-120}
m pause --mode claims --for 2h >/dev/null
echo "   task #$prop2 landed (a doc note; it does not touch the retry logic)"
# After it, the same loop again: five attempts failing the same way, among a few good ones.
"$plant" attempts "$db" --project acmeshop --task 82 --step test --outcome fail \
  --reason "permission denied reading /srv/build/cache" --count 5 --retry-from 1
"$plant" attempts "$db" --project acmeshop --task 83 --step test --outcome green --count 3 --retry-from 1
wait_for 30 "the second verdict" sh -c "$bin/mason --data-dir $root/d --config-dir $root/c health | grep -q \"this fix didn't help\""
m health | grep -A1 "didn't help" | sed 's/^/   /' | head -3
m digest --days 1 | grep -q "this fix didn't help"
[ "$(finding_field 2 "f['verdict']")" = didnt_help ]
[ "$(finding_field 1 "f['verdict']")" = worked ]    # the first verdict stands
echo "   the digest reports both: one fix worked, one didn't help"

echo "== 3. the analyst's findings are held to the same rules"
m resume >/dev/null
m schedule run analyst:watchshop | sed 's/^/   /'
wait_for 60 "the analyst's proposal" sh -c "$bin/mason --data-dir $root/d --config-dir $root/c proposals | grep -q 'Review steps queue behind develop'"
m pause --mode claims --for 2h >/dev/null
refused=$(find "$root/d/t" -name analyst-refused.txt | head -1)
[ -n "$refused" ]
sed 's/^/   refused: /' "$refused"
grep -q "Something feels slow" "$refused"
grep -q "unknown metric" "$refused"
[ "$(m proposals --json | grep -c 'Something feels slow\|Morale is low')" = 0 ]
m health --json | grep -q '"rule": "analyst"'
echo "   one analyst finding became a proposal; the one without evidence and the one with an unknown metric filed nothing"

echo "== 4. triage never answers: it records what it would do, for review"
[ -z "$(m triage list | grep 'would')" ]    # off by default: nothing recorded yet
m resume >/dev/null
m task add "choose the token lifetime" --project askshop --explored --body "x" >/dev/null
asked=$(m task list | sed -n 's/^#\([0-9]*\) .*choose the token lifetime.*/\1/p')
wait_state "$asked" needs_attention 60
wait_for 30 "the triage sample" sh -c "$bin/mason --data-dir $root/d --config-dir $root/c triage list | grep -q 'would answer'"
m pause --mode claims --for 2h >/dev/null
m triage list | sed 's/^/   /'
m triage list | grep -q 'queued'
[ "$(m task show "$asked" --json | jq_state)" = needs_attention ]
echo "   the question is still the owner's (task #$asked waits); the suggestion is queued for review"
tid=$(m triage list --json | sed -n 's/^ *"id": \([0-9]*\),$/\1/p' | head -1)
m triage review "$tid" --disagree
[ "$(m task show "$asked" --json | jq_state)" = needs_attention ]
m triage list | grep -q 'disagree'
m health | grep -q 'Triage: on'
echo "   reviewed (disagree); the task is unchanged"

echo "== 5. the digest: exploring skips by route, forced landings with reasons"
m task add "release blocker" --project acmeshop --explored --body "x" >/dev/null
blocker=$(m task list | sed -n 's/^#\([0-9]*\) .*release blocker.*/\1/p')
m task force "$blocker" --reason "release freeze, owner accepted the risk" >/dev/null
out=$(m digest --days 1)
echo "$out" | sed 's/^/   /'
echo "$out" | grep -q "Exploring skipped for"
echo "$out" | grep -q "flag  *[0-9]* task(s), 0 came back"
echo "$out" | grep -q "Forced landings:"
echo "$out" | grep -q "release freeze, owner accepted the risk"
echo "$out" | grep -q "Tokens per landed task: not recorded"
m digest --days 1 --json | python3 -c "
import json,sys
d=json.load(sys.stdin)
assert d['landed']>=2, d['landed']
assert d['exploring']['skipped']>=2 and d['exploring']['came_back']==0
assert d['median_claim_to_land_ms'] is not None
assert d['tokens_per_landed'] is None
t=d['trace']; assert t['events_replayed']>0 and t['step_attempts_rows']>0
assert d['triage']['answered_by_triage']==0 and d['triage']['suggested']==1
"
echo "   every figure above is a count or sum of recorded rows (digest.trace lists them)"
m task cancel "$blocker" >/dev/null

echo "== 6. base health: only a failure the baseline lacks files a task"
out=$(m schedule run acmeshop/base); echo "   $out"
grep -q "baseline recorded: 1 known failure" <<<"$out"
[ "$(m task list | grep -c 'is red on')" = 0 ]
echo "   first run: old_failure goes into the baseline; nothing filed"
cat > "$root/acmeshop/tests.sh" <<'SH'
echo "1..3"; echo "ok 1 - checkout"; echo "not ok 2 - old_failure"; echo "not ok 3 - new_failure"
SH
git -C "$root/acmeshop" add -A; git -C "$root/acmeshop" -c user.name=t -c user.email=t@t commit -qm "a new failing test"
git -C "$root/acmeshop" push -q origin owner-work:main
out=$(m schedule run acmeshop/base); echo "   $out"
grep -q "1 new" <<<"$out"
[ "$(m task list | grep -c 'is red on')" = 1 ]
m task list | grep 'is red on' | grep -q new_failure
! m task list | grep 'is red on' | grep -q old_failure
m task list | grep 'is red on' | sed 's/^/   /'
out=$(m schedule run acmeshop/base); echo "   $out"
grep -q "0 new" <<<"$out"
[ "$(m task list | grep -c 'is red on')" = 1 ]
echo "   second run: one task, for new_failure only; third run: nothing new"
m schedule list | sed 's/^/   /'
m schedule list | grep -q 'acmeshop/base'
m schedule list | grep -q '^introspect'

echo "== 7. a schedule runs by itself, at low priority, and does not pile up"
kill "$daemon"; wait "$daemon" 2>/dev/null || true; daemon=""
cat >> "$root/c/projects/acmeshop.toml" <<'TOML'

[schedule.logs]
every = "2s"
title = "Look at last night's logs"
body = "A recurring look."
TOML
start_daemon
wait_for 30 "the scheduled task" sh -c "$bin/mason --data-dir $root/d --config-dir $root/c task list | grep -q \"Look at last night's logs\""
sleep 6
[ "$(m task list | grep -c "Look at last night's logs")" = 1 ]
m task list | grep "Look at last night's logs" | sed 's/^/   /'
logs=$(m task list | sed -n "s/^#\([0-9]*\) .*Look at last night's logs.*/\1/p")
m task show "$logs" --json | grep -q '"priority": "low"'
echo "   one task, low priority, three more ticks and it was not filed again while open"

trap - EXIT; cleanup
echo "s22 OK"
