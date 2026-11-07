#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Stage 10 exit test: attention and overrides, through the CLI of a real masond.
#  1. A bad decision card is rejected and rebuilt, so the question is not lost.
#  2. A stale answer is refused, and so is one from someone who is not the owner.
#  3. An answer resumes the task.
#  4. Skip and cancel work through the CLI and leave the right records.
#  5. Pause holds claims back; alerts follow their conditions; the metric counts it all.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
root=$(mktemp -d /tmp/mason-s10.XXXXXX); daemon=""
cleanup() {
  [ -n "$daemon" ] && kill -9 "$daemon" 2>/dev/null || true
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
jq_state() { sed -n 's/^  "state": "\(.*\)",$/\1/p'; }
wait_state() {  # TASK STATE SECONDS
  local end=$((SECONDS + $3))
  until [ "$(m task show "$1" --json 2>/dev/null | jq_state)" = "$2" ]; do
    [ $SECONDS -lt $end ] || { echo "timed out waiting for task $1 to be $2" >&2; m task show "$1" >&2 || true; exit 1; }
    sleep 0.2
  done
}
wait_slow() {  # TASK: until its slow suite is running
  local end=$((SECONDS + 60))
  until ls "$root"/d/h 2>/dev/null | grep -q "^t${1}r[0-9]*-u-slow$"; do
    [ $SECONDS -lt $end ] || { echo "the slow suite never started for task $1" >&2; m task show "$1" >&2; ls "$root/d/h" >&2; tail -20 "$root/masond.log" >&2; exit 1; }
    sleep 0.2
  done
}
task_id() { sed -n 's/.*#\([0-9]*\).*/\1/p'; }
pusher() {  # BRANCH PATH MESSAGE
  [ -d "$root/pusher" ] || git clone -q "$root/origin.git" "$root/pusher" 2>/dev/null
  git -C "$root/pusher" fetch -q origin
  git -C "$root/pusher" checkout -q -B "$1" "origin/$1"
  mkdir -p "$root/pusher/$(dirname "$2")"; cat > "$root/pusher/$2"
  git -C "$root/pusher" add -A
  git -C "$root/pusher" -c user.name=t -c user.email=t@t commit -qm "$3"
  git -C "$root/pusher" push -q origin "HEAD:$1"
}
# A command that must fail, with its message.
refused() { local out; if out=$("$@" 2>&1); then echo "this should have been refused: $*" >&2; exit 1; fi; echo "$out"; }

git init -q --bare -b main "$root/origin.git"
git clone -q "$root/origin.git" "$root/repo" 2>/dev/null
git -C "$root/repo" checkout -q -b main
echo scratch > "$root/repo/README"
git -C "$root/repo" add -A
git -C "$root/repo" -c user.name=t -c user.email=t@t commit -qm init
git -C "$root/repo" push -q origin main
git -C "$root/repo" checkout -q -b owner-work
mkdir -p "$root/c/projects"

# Two scripted agents, one per project (the stand-in counts turns per prompt, so
# each task would start from the top of a script). The `asker` first asks a card
# that does not fit (a pasted log for a question, one-word options, no reason,
# nothing recommended) and then does the work; the `worker` just does the work.
log=$(for i in $(seq 1 25); do printf 'error %s: it broke\\n' "$i"; done)
turn='{"sh": "echo hello > hello.txt && echo $PWD > unique-$(basename \"$PWD\").txt && git add -A && git -c user.name=t -c user.email=t@t commit -qm work"}, {"call": {"tool": "submit_change", "args": {"summary": "did it", "head": "$HEAD"}}}'
cat > "$root/ask.json" <<JSON
{"steps": {"develop": [
  [{"call": {"tool": "ask", "args": {"ask": "Why does the build fail?\\n$log", "options": [{"label": "a"}, {"label": "b"}]}}}],
  [$turn], [$turn]
]}}
JSON
cat > "$root/work.json" <<JSON
{"steps": {"develop": [[$turn], [$turn]]}}
JSON
for p in asker:ask worker:work; do
  cat > "$root/c/projects/${p%%:*}.toml" <<TOML
path = "$root/repo"
base = "main"
remote = "origin"
test = "false"
claude_bin = "$PWD/$bin/fake-claude"
[agent_env]
FAKE_CLAUDE_SCRIPT = "$root/${p##*:}.json"
TOML
done
# develop, then a suite that is slow while a flag file exists.
pusher main .mason/mason.toml "definition" <<'TOML'
schema = 1
start = "develop"

[step.develop]
type = "agent"
category = "developing"
next = "test"

[step.test]
type = "test"
category = "testing"
suites = ["slow"]
on_red = "develop"
max_rounds = 3
next = "land"

[step.land]
type = "landing"
category = "landing"

[suite.slow]
command = "sh slow.sh"
parser = "tap"
min_tests = 1
level = "unit"
TOML
printf '#!/bin/sh\nif [ -f %s/slow.flag ]; then sleep 120; fi\necho 1..1; echo "ok 1 - slow"\n' "$root" | pusher main slow.sh "a suite"

"$bin/masond" --port 0 --data-dir "$root/d" --config-dir "$root/c" --heartbeat-ms 100 --sweep-ms 3600000 --max-concurrent 3 --owner "$owner" 2>>"$root/masond.log" &
daemon=$!
until m status >/dev/null 2>&1; do kill -0 "$daemon" || { cat "$root/masond.log" >&2; exit 1; }; sleep 0.1; done

echo "== 1. a bad card is rejected and rebuilt, so the question is not lost"
q=$(m task add "Asks badly" --project asker --explored --body "x" | task_id)
wait_state "$q" needs_attention 60
m task show "$q" | sed -n '/QUESTION/,/card  /p'
m task show "$q" | grep -q 'QUESTION  Why does the build fail?$'
m task show "$q" | grep -q 'this card was fallback because the agent.s did not fit'
m task show "$q" | grep -q 'ask: .* characters, the limit is 200'
m task show "$q" | grep -q 'exactly one must be recommended'
[ "$(m task show "$q" | grep -c '(recommended)')" = 1 ]
[ "$(m task show "$q" | grep -c '^    | ')" -le 10 ]
m task logs "$q" >/dev/null 2>&1 || m logs "$q" | grep -q 'did not fit the decision-card shape'
echo "(the agent was told, in its own session, that its card did not fit)"

echo "== alerts follow their conditions"
m alerts | grep -q "needs_you .*Task #$q needs a decision: Why does the build fail?"
m alerts | head -3

echo "== 2. a stale answer is refused, and so is one from someone else"
msg=$(refused m task answer "$q" 1 --card 0000000000000000)
echo "$msg"; echo "$msg" | grep -q stale
msg=$(refused m task answer "$q" 1 --as mallory)
echo "$msg"; echo "$msg" | grep -q "not the owner's identity"
[ "$(m task show "$q" --json | jq_state)" = needs_attention ]
msg=$(refused m task answer "$q" 9)
echo "$msg"; echo "$msg" | grep -q "no option 9"

echo "== 3. an answer resumes the task"
m task answer "$q" 1
wait_state "$q" landed 90
echo "task #$q landed after the answer"
m alerts | grep -q "needs_you" && { echo "the alert should have cleared" >&2; exit 1; }
m alerts --all | grep -q "^\. needs_you .*Task #$q"
echo "(the alert cleared with its condition, and is kept in the history)"

echo "== 4. skip and cancel work through the CLI and leave the right records"
touch "$root/slow.flag"
s=$(m task add "Skip the slow suite" --project worker --explored --body "x" | task_id)
wait_slow "$s"
m --reason "not worth two minutes" task skip "$s" --suite slow
wait_state "$s" landed 60
m task show "$s" | grep -E 'tests |debt'
m task show "$s" | grep -q 'tests     unverified: .*slow: skipped by force'
m debt list | grep -q "task #$s .*suite:slow"
c=$(m task add "Cancel me" --project worker --explored --body "x" | task_id)
wait_slow "$c"
m --reason "changed my mind" task cancel "$c"
wait_state "$c" canceled 30
rm -f "$root/slow.flag"
echo "== land now and force are recorded as an override and a debt"
m task land-now "$s" --reason "in a hurry" | tee "$root/ln.out"
grep -q "the landing engine acts on it" "$root/ln.out"
m task force "$s" | head -1
m debt list | grep -E "landing:land_now|landing:force"

echo "== 5. a pause holds claims back, and lifting it lets work go"
m pause --mode claims --for 1h
p=$(m task add "Held back" --project worker --explored --body "x" | task_id)
sleep 1.5
[ "$(m task show "$p" --json | jq_state)" = queued ] || { echo "a paused queue must not claim" >&2; exit 1; }
m pause list
m pause list | grep -q '^claims for everything'
m alerts | grep -q 'paused .*Paused (claims)'
m resume
wait_state "$p" landed 90
m alerts | grep -q paused && { echo "the pause alert should have cleared" >&2; exit 1; }
if m pause --mode claims 2>/dev/null; then echo "a pause without an expiry was accepted" >&2; exit 1; fi

echo "== the metric: interventions per landed task"
m metrics
m metrics --json > "$root/metrics.json"
for kind in answer skip_test cancel land_now force pause resume; do
  grep -q "\"$kind\"" "$root/metrics.json" || { echo "no $kind intervention was recorded" >&2; cat "$root/metrics.json" >&2; exit 1; }
done
grep -q '"refused_answers": 1' "$root/metrics.json"
# Refused answers are not interventions; the others are.
m metrics | grep -q ' per landed task'

# The rest: the card rules, the fallback, the daemon's attention tests.
trap - EXIT; cleanup
if [ -z "${MASON_TESTS_ALREADY_RAN:-}" ]; then
  cargo test -p mason-adapter-api card
  cargo test -p mason-daemon --test attention
fi
echo "s10 OK"
