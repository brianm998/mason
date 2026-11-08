#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s69 exit test: a conflict with the base is not a red round, it has a limit of its
# own, and a retried task gets its rounds again.
set -euo pipefail
cd "$(dirname "$0")/../.."

run() {
    local what=$1; shift
    echo "== $what"
    out=$(cargo test -q "$@" 2>&1) || { echo "$out" >&2; echo "FAILED: $what" >&2; exit 1; }
    echo "$out" | grep -E '^test result' | tail -1
}

run "a conflict at the test step is not a red round" \
    -p mason-daemon --test e2e a_conflict_with_the_base_at_the_test_step_is_not_a_red_round
run "a base the agent never catches up with still fails the task" \
    -p mason-daemon --test e2e a_base_the_agent_never_catches_up_with_still_fails_the_task
run "a retried task gets its test rounds again" \
    -p mason-daemon --test e2e a_retried_task_gets_its_test_rounds_again
run "a conflict goes to the agent to merge (s60)" \
    -p mason-daemon --test e2e a_branch_that_conflicts_with_the_base_goes_to_the_agent_to_merge_and_lands
run "the test step folds in the base (s61)" \
    -p mason-daemon --test e2e the_test_step_folds_in_the_base_so_a_fix_that_landed_meanwhile_counts

echo "s69 OK"
