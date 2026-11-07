#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Task dependencies (s42) exit test: task dependencies. A real masond, a fake GitHub (fake-github, over recorded
# JSON in scripts/exit/fixtures/s42; loopback only) and a bare repository to land on. No network.
#  1. A chain of three issues (101; 102 blocked by 101 through GitHub's own link; 103 with only a
#     "Depends on #102" body line, listed newest first) is imported in order. Only the first runs,
#     and each one starts when the one before lands, promptly (the poll interval is five minutes,
#     so it is the landing that wakes the next, not a poll).
#  2. A cycle between two issues is not imported and the alert names it.
#  3. Removing a blocker on GitHub releases a queued task (the listing's ETag did not change: the
#     dependency lookup noticed). Adding one back after the task started does not pull it back;
#     the drift is alerted.
#  4. A canceled blocker keeps its dependent held and raises an alert.
#  5. `mason task add --after`: held, listed by `task show` (and --json) both ways; a canceled
#     or unknown id is refused naming it.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
root=$(mktemp -d /tmp/mason-s42.XXXXXX); daemon=""; gh=""
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
fx=scripts/exit/fixtures/s42
owner=${USER:-owner}
m() { "$bin/mason" --data-dir "$root/d" --config-dir "$root/c" "$@"; }
fail() { echo "FAILED: $*" >&2; m task list >&2 2>&1 || true; m alerts >&2 2>&1 || true; tail -20 "$root/masond.log" >&2 || true; exit 1; }
state_of() { m task show "$1" --json 2>/dev/null | sed -n 's/^  "state": "\(.*\)",$/\1/p'; }
wait_state() {  # TASK STATE SECONDS
  local end=$((SECONDS + $3))
  until [ "$(state_of "$1")" = "$2" ]; do
    [ $SECONDS -lt $end ] || { m task show "$1" >&2 || true; fail "timed out waiting for task $1 to be $2"; }
    sleep 0.2
  done
}
wait_not_state() {  # TASK STATE SECONDS: the task has left STATE
  local end=$((SECONDS + $3))
  while [ "$(state_of "$1")" = "$2" ]; do
    [ $SECONDS -lt $end ] || fail "task $1 is still $2 after $3 seconds"
    sleep 0.1
  done
}
shows() { m task show "$1" | grep -q "$2" || { m task show "$1" >&2; fail "task $1 does not show: $2"; }; }
not_shows() { ! m task show "$1" | grep -q "$2" || { m task show "$1" >&2; fail "task $1 shows: $2"; }; }
alerts_have() { m alerts | grep -q "$1" || { m alerts >&2; fail "no alert saying: $1"; }; }
gh_set() {  # N: issues-N.json and deps-N.json become what the fake GitHub serves
  sed "s/OWNER/$owner/g" "$fx/issues-$1.json" > "$root/gh/issues.json.tmp"; mv "$root/gh/issues.json.tmp" "$root/gh/issues.json"
  cp "$fx/deps-$1.json" "$root/gh/deps.json"
}
gh_deps() { cp "$fx/deps-$1.json" "$root/gh/deps.json"; }
id_of() { m task list | sed -n "s/^#\([0-9]*\) .* $1\$/\1/p" | head -1; }
tasks() { m task list --json | grep -c '"id"' || true; }

echo "== setup: a bare repository, a pipeline of one command step, and a fake GitHub"
mkdir -p "$root/gh" "$root/c/projects"
gh_set 1
"$bin/fake-github" "$root/gh" "$root/gh.port" 2>"$root/gh.log" &
gh=$!
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
command = "sleep 4; f=$(mktemp ./f-XXXXXX) && echo work > $f && git add -A && git -c user.name=t -c user.email=t@t commit -qm work"
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
cat > "$root/c/projects/web.toml" <<TOML
path = "$root/repo"
base = "main"
remote = "origin"
test = "true"

[tracker]
kind = "github"
api = "http://127.0.0.1:$port"
repo = "o/r"
token = "env:FAKE_GITHUB_TOKEN"
interval_s = 300
TOML
export FAKE_GITHUB_TOKEN=recorded-test-token
"$bin/masond" --port 0 --data-dir "$root/d" --config-dir "$root/c" --heartbeat-ms 100 --sweep-ms 3600000 --owner "$owner" 2>>"$root/masond.log" &
daemon=$!
until m status >/dev/null 2>&1; do kill -0 "$daemon" || { cat "$root/masond.log" >&2; exit 1; }; sleep 0.1; done

echo "== 1. a chain of three issues, listed newest first, is imported in order and runs one at a time"
end=$((SECONDS + 20)); until [ "$(tasks)" = 3 ]; do [ $SECONDS -lt $end ] || fail "the chain was not imported"; sleep 0.2; done
for n in 0 1 2; do m task show $n | head -1; done
m task show 0 | head -1 | grep -q 'Stage A' || fail "task #0 is not issue 101"
m task show 1 | head -1 | grep -q 'Stage B' || fail "task #1 is not issue 102"
m task show 2 | head -1 | grep -q 'Stage C' || fail "task #2 is not issue 103"
echo "   issue 103 (listed first) became task #2, after 102 (#1) and 101 (#0): dependency order, not listing order"
shows 1 'blocked by #0'; shows 0 'blocks    #1'
shows 2 'blocked by #1'; shows 1 'blocks    #2'
shows 2 'held, blocked by #1'
[ "$(state_of 0)" != queued ] || fail "the first task is not running"
[ "$(state_of 1)" = queued ] && [ "$(state_of 2)" = queued ] || fail "a dependent started with its blocker still running"
echo "   only #0 runs; #1 is held by #0 (GitHub's link) and #2 by #1 (a body line)"
m task show 1 --json | tr -d ' \n' | grep -q '"blocked_by":\[0\]' || fail "--json does not list what #1 is blocked by"
m task show 1 --json | tr -d ' \n' | grep -q '"blocks":\[2\]' || fail "--json does not list what #1 blocks"
wait_state 0 landed 120
t0=$SECONDS
wait_not_state 1 queued 10
echo "   #0 landed; #1 started $((SECONDS - t0))s later (a poll is five minutes apart: the landing woke it)"
[ "$(state_of 2)" = queued ] || fail "#2 started before #1 landed"
wait_state 1 landed 120
wait_not_state 2 queued 10
wait_state 2 landed 120
echo "   #1 landed, then #2 started and landed: one at a time, in order"

echo "== 2. a cycle between two issues is not imported, and the alert names it"
gh_set 2
m task refresh 0 >/dev/null
sleep 1
[ "$(tasks)" = 3 ] || fail "an issue in a cycle was imported"
alerts_have '#104 -> #105 -> #104'
m alerts | grep 'cycle' | sed 's/^/   /' | head -2
echo "   neither issue became a task"

echo "== 3. removing a blocker on GitHub releases a queued task; a started one is not pulled back"
m pause --mode claims --for 1h >/dev/null
gh_set 3
m task refresh 0 >/dev/null
end=$((SECONDS + 20)); until [ "$(tasks)" = 7 ]; do [ $SECONDS -lt $end ] || fail "issues 106 to 109 were not imported"; sleep 0.2; done
# Imported in dependency order within one listing: a blocker first, whatever the listing order.
h=$(id_of "Holds 107"); r=$(id_of "Released by an edit"); c=$(id_of "Will be closed"); l=$(id_of "Late after canceled")
[ -n "$h$r$c$l" ] && [ "$h" -lt "$r" ] && [ "$c" -lt "$l" ] || fail "the tasks for issues 106 to 109 are out of order ($h $r $c $l)"
shows "$r" "blocked by #$h"
echo "   #$r (issue 107) is held by #$h (issue 106); #$l (issue 109) by #$c (issue 108)"
gh_deps 4
m task refresh 0 >/dev/null
not_shows "$r" 'blocked by #'
! m task show "$r" | grep -q 'held' || fail "#$r is still held"
echo "   the dependency came off on GitHub (the listing was not modified, the dependency lookup saw it): #$r is released"
m resume --mode claims >/dev/null
wait_not_state "$r" queued 20
gh_deps 5
m task refresh 0 >/dev/null
not_shows "$r" 'blocked by #'
alerts_have 'not pulled back'
echo "   a blocker added on GitHub after #$r started: it is not pulled back, and the alert says so"
m alerts | grep 'not pulled back' | sed 's/^/   /' | head -1

echo "== 4. a canceled blocker keeps the dependent held and raises an alert"
m pause --mode claims --for 1h >/dev/null
gh_set 4
m task refresh 0 >/dev/null
sleep 1
[ "$(state_of "$c")" = canceled ] || fail "closing issue 108 on GitHub did not cancel task #$c ($(state_of "$c"))"
shows "$l" 'held'
shows "$l" "blocked by #$c"
alerts_have 'was canceled'
m alerts | grep 'was canceled' | sed 's/^/   /' | head -1
echo "   issue 108 was closed as not planned: its task is canceled, #$l stays held and someone is told"
m resume --mode claims >/dev/null
wait_state "$h" landed 120
wait_state "$r" landed 120
[ "$(state_of "$l")" = queued ] || fail "#$l left queued although its blocker was canceled"
shows "$l" 'held'
echo "   with everything else landed, #$l is still held"

echo "== 5. mason task add --after"
out=$(m task add "waits for 3 and 4" --project web --explored --after "$h,$r" 2>&1) || fail "--after on landed tasks: $out"
id=$(echo "$out" | sed -n 's/^Filed task #\([0-9]*\).*/\1/p')
echo "   $out (both have landed: already satisfied)"
if m task add "after a canceled one" --project web --after "$c" >"$root/refused" 2>&1; then fail "--after on a canceled task was accepted"; fi
grep -q "#$c" "$root/refused" || fail "the refusal does not name #$c: $(cat "$root/refused")"
echo "   refused: $(tail -1 "$root/refused")"
if m task add "after nothing" --project web --after 99 >"$root/refused" 2>&1; then fail "--after on an unknown task was accepted"; fi
grep -q '#99' "$root/refused" || fail "the refusal does not name #99"
echo "   refused: $(tail -1 "$root/refused")"
m pause --mode claims --for 1h >/dev/null
out=$(m task add "waits for the late one" --project web --explored --after "$l") || fail "filing after #$l"
id2=$(echo "$out" | sed -n 's/^Filed task #\([0-9]*\).*/\1/p')
shows "$id2" "blocked by #$l"; shows "$l" "blocks    #$id2"
m task show "$id2" --json | tr -d ' \n' | grep -q "\"blocked_by\":\\[$l\\]" || fail "--json blocked_by"
echo "   task #$id2 is held by #$l, and #$l shows that it blocks #$id2"
m resume --mode claims >/dev/null

echo "== the tracker was asked politely"
echo "   requests to the fake GitHub: $(wc -l < "$root/gh/requests.log" | tr -d ' ') (listings: $(grep -c 'GET /repos/o/r/issues?' "$root/gh/requests.log"), dependency lookups: $(grep -c 'blocked_by' "$root/gh/requests.log"))"

trap - EXIT; cleanup
echo "s42 OK"
