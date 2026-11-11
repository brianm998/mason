#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Self-hosting weekly cap (s44) exit test: the cap counts only work MASON starts for itself.
# A real masond on a project with [self] per_week = 3, and a fake GitHub (loopback only).
#  1. Outside issues the owner labeled (work MASON takes in) fill the week: the 4th is not imported.
#  2. `mason task add` as the owner is accepted past the cap; `mason self` shows the split.
#  3. An issue by an owner name is imported past the cap.
#  4. Cancelling a task before any step started frees a slot: the 4th outside issue comes in.
#  5. max_open no longer refuses the owner (s48): only work MASON starts for itself is counted.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
root=$(mktemp -d /tmp/mason-s44.XXXXXX); daemon=""; gh=""
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

echo "== setup: a bare repository, a project with [self] per_week = 3, and a fake GitHub"
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
per_week = 3

[tracker]
kind = "github"
api = "http://127.0.0.1:$port"
repo = "o/r"
token = "env:FAKE_GITHUB_TOKEN"
interval_s = 300
# Imports start proposed by default (since b14ad6a); the cap is on work taken in.
import_as = "queued"
TOML
export FAKE_GITHUB_TOKEN=recorded-test-token
"$bin/masond" --port 0 --data-dir "$root/d" --config-dir "$root/c" --heartbeat-ms 100 --sweep-ms 3600000 --owner "$owner" 2>>"$root/masond.log" &
daemon=$!
until m status >/dev/null 2>&1; do kill -0 "$daemon" || { cat "$root/masond.log" >&2; exit 1; }; sleep 0.1; done
# Nothing starts: every task stays queued, so none has begun a step.
m pause --mode claims --for 1h >/dev/null
# A seed task (the owner's, so not counted), to ask the tracker through.
m task add "seed" --project self --explored >/dev/null || fail "the seed task"

echo "== 1. three labeled outside issues fill the week; the fourth is not imported"
gh_issues "$(issue 201 stranger "$M")" "$(issue 202 stranger "$M")" "$(issue 203 stranger "$M")" "$(issue 204 stranger "$M")"
refresh
[ "$(tasks)" = 4 ] || fail "expected the seed and 3 imported outside issues, got $(tasks)"
self_line | sed 's/^/   /'
self_line | grep -q '3 of 3 this week (1 of yours not counted)' || fail "mason self does not show 3 of 3 (1 of yours not counted)"
! m self | grep -q 'filing is blocked' || fail "mason self says filing is blocked, but the owner can still file"

echo "== 2. the owner files with mason task add past the cap"
out=$(m task add "my own task" --project self --explored 2>&1) || fail "the owner's task was refused: $out"
echo "   $out"
self_line | sed 's/^/   /'
self_line | grep -q '3 of 3 this week (2 of yours not counted)' || fail "the split is not shown after the owner's task"

echo "== 3. an issue by an owner name is imported past the cap"
gh_issues "$(issue 301 "$owner" '[]')" "$(issue 201 stranger "$M")" "$(issue 202 stranger "$M")" "$(issue 203 stranger "$M")" "$(issue 204 stranger "$M")"
refresh
[ "$(tasks)" = 6 ] || fail "expected 6 tasks (3 outside, 3 the owner's), got $(tasks)"
self_line | grep -q '3 of 3 this week (3 of yours not counted)' || fail "the split is not shown after the owner's issue"
self_line | sed 's/^/   /'

echo "== 4. cancelling a task before it started frees a slot"
m task cancel 1 >/dev/null || fail "cancel"
refresh
[ "$(tasks)" = 7 ] || fail "issue 204 was not imported after the cancel ($(tasks) tasks)"
self_line | grep -q '3 of 3 this week (4 of yours not counted)' || fail "the cancelled task was not moved out of the count"
self_line | sed 's/^/   /'

echo "== 5. max_open does not refuse the owner (s48)"
for n in 1 2 3 4; do m task add "owner task $n" --project self --explored >/dev/null || fail "the owner's task $n was refused"; done
m self | grep -q 'yours open, not limited' || fail "mason self does not show the owner's open tasks as not limited"
echo "   $(m self | grep '^self:')"

trap - EXIT; cleanup
echo "s44 OK"
