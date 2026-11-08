#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Tracker imports (s65) exit test: imports start proposed, a refused import is visible,
# a hand-filed task is adopted. A real masond, a fake GitHub per project, command steps.
# Project "self" (import_as default = proposed, every limit full):
#  1. `task add --proposed` is accepted at max_open; five issues become five proposed
#     tasks, no alert; approving one runs it to landed.
#  2. Closing an issue on the fake host cancels its proposed task.
#  3. A task titled "(GitHub #N)" is adopted: no second task; closing the issue cancels it.
#  4. Cancelling a linked task raises the relink alert; `task relink` (owner only) clears it.
# Project "queued" (import_as = "queued", [self] max_open = 1):
#  5. Two issues are not imported; the alert names them and the limit, `--pending-issues`
#     lists them; both clear once they import.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
root=$(mktemp -d /tmp/mason-s65.XXXXXX); daemon=""; gh=""; gh2=""
cleanup() {
  [ -n "$daemon" ] && kill -9 "$daemon" 2>/dev/null || true
  [ -n "$gh" ] && kill -9 "$gh" 2>/dev/null || true
  [ -n "$gh2" ] && kill -9 "$gh2" 2>/dev/null || true
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
fail() { echo "FAILED: $*" >&2; set +e; m task list >&2 2>&1; m alerts >&2 2>&1; tail -20 "$root/masond.log" >&2; exit 1; }
issue() {  # NUMBER STATE
  printf ' {"number": %s, "node_id": "I_%s", "title": "Issue %s", "body": "text", "state": "%s", "user": {"login": "%s"}, "labels": []}' "$1" "$1" "$1" "$2" "$owner"
}
gh_issues() {  # DIR then issue objects
  local dir=$1; shift; local out="[" sep=""
  for i in "$@"; do out="$out$sep$i"; sep=","; done
  printf '%s]\n' "$out" > "$dir/issues.json.tmp"; mv "$dir/issues.json.tmp" "$dir/issues.json"
}
state_of() { m task show "$1" --json 2>/dev/null | sed -n 's/^  "state": "\(.*\)",$/\1/p'; }
wait_state() { local end=$((SECONDS + $3)); until [ "$(state_of "$1")" = "$2" ]; do [ $SECONDS -lt $end ] || fail "timed out waiting for task $1 to be $2 (is $(state_of "$1"))"; sleep 0.2; done; }
id_of() { m task list | grep "$1" | head -1 | sed -E 's/^#?([0-9]+).*/\1/'; }
count_of() { m task list | awk -v p="$1" '/^#/ && index($0, p) { n++ } END { print n + 0 }'; }
alerts_have() { m alerts | grep -q "$1" || { m alerts >&2; fail "no alert saying: $1"; }; }
alerts_lack() { ! m alerts | grep -q "$1" || { m alerts >&2; fail "an alert still says: $1"; }; }

echo "== setup: two repositories, two projects, two fake GitHubs"
mkdir -p "$root/gh" "$root/gh2" "$root/c/projects"
for g in gh gh2; do echo '[]' > "$root/$g/issues.json"; echo '{}' > "$root/$g/deps.json"; done
"$bin/fake-github" "$root/gh" "$root/gh.port" 2>"$root/gh.log" & gh=$!
"$bin/fake-github" "$root/gh2" "$root/gh2.port" 2>"$root/gh2.log" & gh2=$!
until [ -s "$root/gh.port" ] && [ -s "$root/gh2.port" ]; do sleep 0.05; done
port=$(cat "$root/gh.port"); port2=$(cat "$root/gh2.port")
hold="$root/hold"; touch "$hold"
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
command = "while [ -e HOLDFILE ]; do sleep 0.2; done; f=$(mktemp ./f-XXXXXX) && echo work > $f && git add -A && git -c user.name=t -c user.email=t@t commit -qm work"
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
sed -i.bak "s|HOLDFILE|$hold|" "$root/repo/.mason/mason.toml"; rm "$root/repo/.mason/mason.toml.bak"
printf 'mode = "direct"\n' > "$root/repo/.mason/landing.toml"
echo base > "$root/repo/base.txt"
git -C "$root/repo" add -A
git -C "$root/repo" -c user.name=t -c user.email=t@t commit -qm "pipeline"
git -C "$root/repo" push -q origin main
git clone -q "$root/origin.git" "$root/repo2" 2>/dev/null
git -C "$root/repo2" checkout -q main
cat > "$root/c/projects/self.toml" <<TOML
path = "$root/repo"
base = "main"
remote = "origin"
test = "true"

[self]
max_open = 1
per_week = 1
max_steps = 1

[tracker]
kind = "github"
api = "http://127.0.0.1:$port"
repo = "o/r"
token = "env:FAKE_GITHUB_TOKEN"
interval_s = 300
TOML
cat > "$root/c/projects/queued.toml" <<TOML
path = "$root/repo2"
base = "main"
remote = "origin"
test = "true"

[self]
max_open = 1

[tracker]
kind = "github"
api = "http://127.0.0.1:$port2"
repo = "o/r2"
token = "env:FAKE_GITHUB_TOKEN"
interval_s = 300
import_as = "queued"
TOML
export FAKE_GITHUB_TOKEN=recorded-test-token
"$bin/masond" --data-dir "$root/d" --config-dir "$root/c" --heartbeat-ms 100 --sweep-ms 3600000 --owner "$owner" 2>>"$root/masond.log" &
daemon=$!
until m status >/dev/null 2>&1; do kill -0 "$daemon" || { cat "$root/masond.log" >&2; exit 1; }; sleep 0.1; done

echo "== 1. the limits are full; a proposed filing and five imports are still accepted"
m task add "seed" --project self --explored >/dev/null || fail "the seed task"
wait_state 0 developing 60
if out=$(m task add "queued one" --project self --explored 2>&1); then fail "a queued filing was accepted at max_open"; fi
echo "$out" | grep -q 'limit is 1' || fail "the refusal does not name the limit: $out"
out=$(m task add "a proposal" --project self --proposed 2>&1) || fail "--proposed was refused at max_open: $out"
echo "   --proposed accepted at max_open; a queued filing is still refused"
refresh() { m task refresh "$1" >/dev/null; sleep 1; }
G=$root/gh
gh_issues $G "$(issue 401 open)" "$(issue 402 open)" "$(issue 403 open)" "$(issue 404 open)" "$(issue 405 open)"
refresh 0
for n in 401 402 403 404 405; do
  t=$(id_of "Issue $n"); [ -n "$t" ] || fail "issue $n has no task"
  [ "$(state_of "$t")" = proposed ] || fail "issue $n's task #$t is $(state_of "$t"), not proposed"
done
echo "   five issues, five proposed tasks"
alerts_lack 'not imported'
m task list --pending-issues | grep -q 'no issues waiting' || fail "--pending-issues lists something"

echo "== approve one and watch it run"
t401=$(id_of "Issue 401")
m task approve "$t401" >/dev/null
rm -f "$hold"
wait_state "$t401" landed 120
echo "   task #$t401 (issue 401) landed after approval"
for n in 403 404 405; do [ "$(state_of "$(id_of "Issue $n")")" = proposed ] || fail "issue $n started without approval"; done

echo "== 2. closing an issue on the host cancels its proposed task"
gh_issues $G "$(issue 401 open)" "$(issue 402 closed)" "$(issue 403 open)" "$(issue 404 open)" "$(issue 405 open)"
refresh 0
wait_state "$(id_of 'Issue 402')" canceled 30
echo "   issue 402 closed, its task canceled"

echo "== 3. a hand-filed task titled (GitHub #N) is adopted"
before=$(m task list | grep -c '^#')
m task add "Hand filed (GitHub #406)" --project self --proposed >/dev/null
gh_issues $G "$(issue 401 open)" "$(issue 402 closed)" "$(issue 403 open)" "$(issue 404 open)" "$(issue 405 open)" "$(issue 406 open)"
refresh 0
after=$(m task list | grep -c '^#')
[ "$after" = $((before + 1)) ] || fail "issue 406 was imported again: $before tasks before, $after after"
[ "$(count_of '406')" = 1 ] || fail "two tasks mention 406"
hand=$(id_of 'Hand filed')
gh_issues $G "$(issue 401 open)" "$(issue 402 closed)" "$(issue 403 open)" "$(issue 404 open)" "$(issue 405 open)" "$(issue 406 closed)"
refresh 0
wait_state "$hand" canceled 30
echo "   task #$hand was linked to issue 406 (closing the issue cancelled it); no duplicate"

echo "== 4. a cancelled linked task raises the relink alert; relink clears it"
t403=$(id_of 'Issue 403')
m task cancel "$t403" >/dev/null
wait_state "$t403" canceled 30
refresh 0
alerts_have 'mason task relink'
alerts_have '#403'
m task list --pending-issues | grep -q '#403' || fail "--pending-issues does not list 403"
m alerts | grep 'relink' | sed 's/^/   /' | head -2
new=$(m task add "Replacement for 403" --project self --proposed | sed -E 's/^Filed task #([0-9]+).*/\1/')
if out=$(MASON_AS=stranger m task relink "$new" --issue 403 2>&1); then fail "a non-owner relinked: $out"; fi
echo "   a non-owner is refused"
if out=$(m task relink "$t403" --issue 403 2>&1); then fail "relinked to a cancelled task"; fi
m task relink "$new" --issue 403 >/dev/null || fail "the owner's relink failed"
refresh 0
alerts_lack 'mason task relink'
m task list --pending-issues | grep -q 'no issues waiting' || fail "--pending-issues still lists something"
echo "   relinked task #$new to issue 403; alert and list cleared"

echo "== 5. queued project: a refused import is visible and clears when they import"
touch "$hold"
m task add "seed2" --project queued --explored >/dev/null || fail "the second seed"
s2=$(id_of seed2); wait_state "$s2" developing 60
Q=$root/gh2
gh_issues $Q "$(issue 501 open)" "$(issue 502 open)"
refresh "$s2"
alerts_have '2 issues not imported (#501, #502)'
alerts_have 'limit is 1'
m alerts | grep 'not imported' | sed 's/^/   /' | head -1
m task list --pending-issues | grep -q '#501' || fail "--pending-issues does not list 501"
m task list --pending-issues | grep -q '#502' || fail "--pending-issues does not list 502"
rm -f "$hold"
wait_state "$s2" landed 120
end=$((SECONDS + 120))
until [ -n "$(id_of 'Issue 501')" ] && [ -n "$(id_of 'Issue 502')" ] && ! m alerts | grep -q 'not imported'; do
  [ $SECONDS -lt $end ] || fail "the issues were not both imported and the alert cleared"
  m task refresh "$s2" >/dev/null; sleep 1
done
[ "$(state_of "$(id_of 'Issue 501')")" != proposed ] || fail "a queued import was left proposed"
echo "   both imported (queued), alert cleared"

trap - EXIT; cleanup
echo "s65 OK"
