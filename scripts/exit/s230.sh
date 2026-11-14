#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s230 exit test: the known-red ledger.
#  1. Only test names enter it; an entry is added once and lifted only by a pass.
#  2. A baseline red on the suites path adds one entry and files one fix task.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
run() {
    local what=$1; shift
    echo "== $what"
    out=$(cargo test -q "$@" 2>&1) || { echo "$out" >&2; echo "FAILED: $what" >&2; exit 1; }
    echo "$out" | grep -E '^test result' | tail -1
}
run "1a. test-name filter" -p mason-testing only_test_names_enter_the_ledger
run "1b. ledger store" -p mason-store the_ledger_adds_one_entry_per_test_and_lifts_only_what_passed
run "2. daemon: baseline red, one entry, one fix task" \
    -p mason-daemon --test testing a_baseline_failure_is_excused_but_a_new_failure_is_the_tasks
echo "s230 OK"
