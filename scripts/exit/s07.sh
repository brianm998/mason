#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Stage 7 exit test: run two different definitions through the fake agent,
# confirm a flagged task goes straight to developing while an unflagged vague
# one explores, and confirm an invalid definition edit leaves the old one running.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
root=$(mktemp -d /tmp/mason-s07.XXXXXX); daemon=""
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
m() { "$bin/mason" --data-dir "$root/d" --config-dir "$root/c" "$@"; }
jq_state() { sed -n 's/^  "state": "\(.*\)",$/\1/p'; }
wait_state() {  # TASK STATE SECONDS
  local end=$((SECONDS + $3))
  until [ "$(m task show "$1" --json 2>/dev/null | jq_state)" = "$2" ]; do
    [ $SECONDS -lt $end ] || { echo "timed out waiting for task $1 to be $2" >&2; m task show "$1" >&2 || true; exit 1; }
    sleep 0.2
  done
}
# The step names a task ran, in order, space separated.
steps_of() { m task show "$1" --json | grep '"name":' | sed 's/.*"name": "\(.*\)".*/\1/' | tr '\n' ' ' | sed 's/ $//'; }
push() {  # commit a file to origin's main as someone else would
  [ -d "$root/pusher" ] || git clone -q "$root/origin.git" "$root/pusher" 2>/dev/null
  git -C "$root/pusher" pull -q origin main
  mkdir -p "$root/pusher/$(dirname "$1")"; cat > "$root/pusher/$1"
  git -C "$root/pusher" add -A
  git -C "$root/pusher" -c user.name=t -c user.email=t@t commit -qm "$2"
  git -C "$root/pusher" push -q origin HEAD:main
}

# A scratch repository with a bare origin, and a scripted stand-in for the agent
# that gives every task a file of its own to commit.
git init -q --bare -b main "$root/origin.git"
git clone -q "$root/origin.git" "$root/repo" 2>/dev/null
git -C "$root/repo" checkout -q -b main
echo scratch > "$root/repo/README"
git -C "$root/repo" add -A
git -C "$root/repo" -c user.name=t -c user.email=t@t commit -qm init
git -C "$root/repo" push -q origin main
git -C "$root/repo" checkout -q -b owner-work
mkdir -p "$root/c/projects"
cat > "$root/script.json" <<'JSON'
{"steps": {
  "explore": [[{"call": {"tool": "submit_plan", "args": {"summary": "add hello.txt", "files": ["hello.txt"], "approach": "write the file", "acceptance_criteria": ["hello.txt exists"]}}}]],
  "develop": [[
    {"sh": "echo hello > hello.txt && date +%s%N > unique-$(basename \"$PWD\").txt && git add -A && git -c user.name=t -c user.email=t@t commit -qm 'add hello'"},
    {"call": {"tool": "submit_change", "args": {"summary": "added hello.txt", "head": "$HEAD"}}}
  ]]
}}
JSON
cat > "$root/c/projects/scratch.toml" <<TOML
path = "$root/repo"
base = "main"
remote = "origin"
test = "test -f hello.txt"
claude_bin = "$PWD/$bin/fake-claude"
[agent_env]
FAKE_CLAUDE_SCRIPT = "$root/script.json"
TOML

# ---- Definition one: explore, develop, test, land, with a way to skip exploring.
cat > "$root/def1.toml" <<'TOML'
schema = 1
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
max_rounds = 3
on_red = "develop"
next = "land"

[step.land]
type = "landing"
category = "landing"
TOML
# ---- Definition two: no exploring at all, and a lint step in front of the tests.
cat > "$root/def2.toml" <<'TOML'
schema = 1
start = "develop"

[step.develop]
type = "agent"
category = "developing"
next = "lint"

[step.lint]
type = "command"
category = "testing"
command = "test -f hello.txt"
max_rounds = 2
on_red = "develop"
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

echo "== both definitions check out offline, with their dry runs"
m definition check "$root/def1.toml" | head -4
m definition check "$root/def2.toml" | head -3

push .mason/mason.toml "pipeline: definition one" < "$root/def1.toml"
"$bin/masond" --port 0 --data-dir "$root/d" --config-dir "$root/c" --heartbeat-ms 100 --sweep-ms 3600000 --reload-interval-ms 200 --fetch-interval-ms 300 2>>"$root/masond.log" &
daemon=$!
until m status >/dev/null 2>&1; do kill -0 "$daemon" || { cat "$root/masond.log" >&2; exit 1; }; sleep 0.1; done
m doctor | grep -E 'mason.toml|FAIL' || true

echo "== definition one: a flagged task skips exploring, a vague one explores"
m task add "Already a plan" --explored --body "Do it." > /dev/null
m task add "Make it better" --body "Make the thing better somehow." > /dev/null
wait_state 0 landed 90; wait_state 1 landed 90
flagged=$(steps_of 0); vague=$(steps_of 1)
echo "flagged task ran: $flagged"
echo "vague task ran:   $vague"
[ "$flagged" = "develop test land" ] || { echo "the flagged task should have started at develop" >&2; exit 1; }
[ "$vague" = "explore develop test land" ] || { echo "the vague task should have explored" >&2; exit 1; }
m task show 0 | grep -q 'skipped exploring: filed as already explored'
m task show 1 | grep -q 'explored first'
h1=$(m definition show | sed -n 's/^definition \([0-9a-f]*\).*/\1/p')

echo "== definition two replaces it for new tasks"
push .mason/mason.toml "pipeline: definition two" < "$root/def2.toml"
# The watcher may get there first; either way the definition changes.
m definition reload | grep -qE 'Applied|Nothing changed'
m task add "Runs under definition two" --body "x" > /dev/null
wait_state 2 landed 90
two=$(steps_of 2); echo "task under definition two ran: $two"
[ "$two" = "develop lint test land" ] || { echo "expected the lint graph" >&2; exit 1; }
[ "$(steps_of 0)" = "develop test land" ]   # earlier tasks keep their record
h2=$(m definition show | sed -n 's/^definition \([0-9a-f]*\).*/\1/p')
[ "$h1" != "$h2" ]

echo "== an invalid edit is refused and the old definition keeps running"
sed 's/max_rounds = 2$//; s/^start = /strat = /' "$root/def2.toml" | push .mason/mason.toml "pipeline: broken" 
if m definition reload > "$root/reload.out"; then echo "a broken definition was accepted" >&2; exit 1; fi
cat "$root/reload.out"
grep -q 'Refused; the current definition keeps running' "$root/reload.out"
grep -q 'did you mean `start`' "$root/reload.out"
m definition show | grep -q REFUSED
[ "$(m definition show | sed -n 's/^definition \([0-9a-f]*\).*/\1/p')" = "$h2" ]   # still definition two
m status | grep -q REFUSED
m task add "Filed while the edit is refused" --body "x" > /dev/null
wait_state 3 landed 90
[ "$(steps_of 3)" = "develop lint test land" ] || { echo "the task should have run under the last good definition" >&2; exit 1; }
echo "task 3 ran under the last good definition: $(steps_of 3)"

echo "== a good edit clears the refusal"
push .mason/mason.toml "pipeline: fixed" < "$root/def2.toml"
m definition reload | grep -qE 'Nothing changed|Applied'
! m definition show | grep -q REFUSED
echo "refusal cleared"

# The rest: the definition crate's tests (validator rules, property tests, clarity,
# layers, dry run) and the daemon's definition end-to-end tests.
trap - EXIT; cleanup
if [ -z "${MASON_TESTS_ALREADY_RAN:-}" ]; then
  cargo test -p mason-definition
  cargo test -p mason-daemon --test definitions
fi
echo "s07 OK"
