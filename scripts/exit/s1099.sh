#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1099 exit test: a red s798 names the task state and the last log line on the line the
# runner keeps as `cause s798:` (#1119). s798's `fail` and the runner's `cause_of` are run
# as written, against a fake `m`.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
root=$(mktemp -d /tmp/mason-s1099.XXXXXX)
trap 'rm -rf "$root"' EXIT
bash -n scripts/exit/s798.sh
sed -n '/^jq_state() /p; /^fail() {/,/^}/p' scripts/exit/s798.sh > "$root/fail.sh"
sed -n '/^cause_of() {/,/^}/p' scripts/run-exit-scripts.sh > "$root/cause.sh"
: > "$root/masond.log"
. "$root/cause.sh"

# $1: what the fake `m` does. Leaves fail's stderr in $root/out.
run_fail() {
  local rc=0
  ( eval "$1"; . "$root/fail.sh"; fail "run_check never named the failing test" ) 2>"$root/out" || rc=$?
  [ "$rc" -eq 1 ] || { echo "FAILED: fail exited $rc, not 1" >&2; cat "$root/out" >&2; exit 1; }
}
expect() {
  [ "$1" = "$2" ] || { echo "FAILED: expected '$2', got '$1'" >&2; exit 1; }
}

echo "== a task with no state and no logs"
run_fail 'm() { return 1; }'
want='FAILED: run_check never named the failing test (task state: unknown; last log line: none)'
expect "$(head -n 1 "$root/out")" "$want"
expect "$(cause_of "$root/out")" "$want"

echo "== the state and the last non-empty log line are named"
run_fail 'm() { case "$*" in
  "task show 0 --json") printf "{\n  \"state\": \"testing\",\n}\n" ;;
  "logs 0") printf "first line\nwaiting for a slot\n\n" ;;
esac; }'
want='FAILED: run_check never named the failing test (task state: testing; last log line: waiting for a slot)'
expect "$(head -n 1 "$root/out")" "$want"
expect "$(cause_of "$root/out")" "$want"

grep -q '^Summary: ' docs/stages/s1099.md
echo "s1099 passed"
