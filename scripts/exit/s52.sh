#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Self-hosting max_steps (s52) exit test: the step cap counts only work MASON starts for itself.
# A real masond on a project with [self] max_steps = 1, command steps in the developing category
# (the cap counts step runs by category, so the count needs no agent), and a fake GitHub.
#  1. A labeled outside issue runs its step (1 of 1 counted); a second one is not imported.
#  2. Owner tasks (seed, CLI, tracker by an owner name) are accepted and run past the cap;
#     mason self shows the split.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
root=$(mktemp -d /tmp/mason-s52.XXXXXX); daemon=""; gh=""
cleanup() {
  [ -n "$daemon" ] && kill -9 "$daemon" 2>/dev/null || true
  [ -n "$gh" ] && kill -9 "$gh" 2>/dev/null || true
  # Verified before it is killed: a stale pid in a registry may belong to someone else now.
  for hd in "$root"/d/h; do target/debug/step-host-ctl reap "$hd" >/dev/null 2>&1; done
  rm -rf "$root"
}
trap cleanup EXIT
cargo build -q --workspace --bins
bin=target/debug
owner=${USER:-owner}
m() { "$bin/mason" --data-dir "$root/d" --config-dir "$root/c" "$@"; }
fail() { echo "FAILED: $*" >&2; m task list >&2 2>&1 || true; m self >&2 2>&1 || true; tail -20 "$root/masond.log" >&2 || true; exit 1; }
tasks() { m task list --json | grep -c '"id"' || true; }
issue() {  # NUMBER AUTHOR LABEL-JSON
  printf ' {"number": %s, "node_id": "I_%s", "title": "Issue %s", "body": "text", "state": "open", "user": {"login": "%s"}, "labels": %s}' "$1" "$1" "$1" "$2" "$3"
}
gh_issues() {  # issue objects as arguments
  local out="[" sep=""
  for i in "$@"; do out="$out$sep$i"; sep=","; done
  printf '%s]\n' "$out" > "$root/gh/issues.json.tmp"; mv "$root/gh/issues.json.tmp" "$root/gh/issues.json"
}
refresh() { m task refresh 0 >/dev/null; sleep 1; }  # task #0 is the owner's seed task
self_line() { m self | grep '^self:'; }
M='[{"name": "mason"}]'

echo "== setup: a bare repository, a project with [self] max_steps = 1, and a fake GitHub"
mkdir -p "$root/gh" "$root/c/projects"
echo '[]' > "$root/gh/issues.json"; echo '{}' > "$root/gh/deps.json"
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
command = "f=$(mktemp ./f-XXXXXX) && echo work > $f && git add -A && git -c user.name=t -c user.email=t@t commit -qm work"
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
cat > "$root/c/projects/self.toml" <<TOML
path = "$root/repo"
base = "main"
remote = "origin"
test = "true"

[self]
max_open = 7
per_week = 50
max_steps = 1

[tracker]
kind = "github"
api = "http://127.0.0.1:$port"
repo = "o/r"
token = "env:FAKE_GITHUB_TOKEN"
interval_s = 300
# Imports start proposed by default (since b14ad6a); this one is meant to run.
import_as = "queued"
TOML
export FAKE_GITHUB_TOKEN=recorded-test-token
"$bin/masond" --port 0 --data-dir "$root/d" --config-dir "$root/c" --heartbeat-ms 100 --sweep-ms 3600000 --owner "$owner" 2>>"$root/masond.log" &
daemon=$!
until m status >/dev/null 2>&1; do kill -0 "$daemon" || { cat "$root/masond.log" >&2; exit 1; }; sleep 0.1; done
# A seed task (the owner's, so not counted), to ask the tracker through.
m task add "seed" --project self --explored >/dev/null || fail "the seed task"

state_of() { m task show "$1" --json 2>/dev/null | sed -n 's/^  "state": "\(.*\)",$/\1/p'; }
wait_state() { local end=$((SECONDS + $3)); until [ "$(state_of "$1")" = "$2" ]; do [ $SECONDS -lt $end ] || fail "timed out waiting for task $1 to be $2"; sleep 0.2; done; }

echo "== 1. a labeled outside issue runs its step and fills the cap; the next is not imported"
wait_state 0 landed 120
gh_issues "$(issue 201 stranger "$M")"
refresh
wait_state 1 landed 120
self_line | sed 's/^/   /'
self_line | grep -q '1 of 1 agent steps in 24 h (1 of yours not counted)' || fail "mason self does not show the split after the outside step"
gh_issues "$(issue 201 stranger "$M")" "$(issue 202 stranger "$M")"
refresh
[ "$(tasks)" = 2 ] || fail "issue 202 was imported past the step cap ($(tasks) tasks)"
echo "   the second outside issue was not imported"

echo "== 2. owner tasks are accepted and run past the cap"
out=$(m task add "my own task" --project self --explored 2>&1) || fail "the owner's task was refused: $out"
wait_state 2 landed 120
gh_issues "$(issue 301 "$owner" '[]')" "$(issue 201 stranger "$M")" "$(issue 202 stranger "$M")"
refresh
wait_state 3 landed 120
[ "$(tasks)" = 4 ] || fail "expected 4 tasks, got $(tasks)"
self_line | sed 's/^/   /'
self_line | grep -q '1 of 1 agent steps in 24 h (3 of yours not counted)' || fail "the split is not shown after the owner's tasks"
! m self | grep -q 'filing is blocked' || fail "mason self says filing is blocked for the owner"

trap - EXIT; cleanup
echo "s52 OK"
