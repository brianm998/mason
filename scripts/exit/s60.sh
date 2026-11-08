#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s60 exit test: a base conflict goes to the develop agent and the task lands; run_check runs
# the overridden test command; the tests that failed only on a busy machine check their property.
set -euo pipefail
cd "$(dirname "$0")/../.."

run() {
    local what=$1; shift
    echo "== $what"
    out=$(cargo test -q "$@" 2>&1) || { echo "$out" >&2; echo "FAILED: $what" >&2; exit 1; }
    echo "$out" | grep -E '^test result' | tail -1
}

run "a base conflict goes to the agent, which merges, and the task lands" \
    -p mason-daemon --test e2e a_branch_that_conflicts_with_the_base_goes_to_the_agent_to_merge_and_lands
run "run_check runs the test command the owner overrode at runtime" \
    -p mason-daemon --test e2e run_check_runs_the_test_command_the_owner_overrode_at_runtime
run "a claim is never handed out without a free session" \
    -p mason-daemon --test e2e admission_respects_the_concurrency_limit
run "the hand-off is driven by the step's end" \
    -p mason-daemon --test protocol when_a_step_ends_the_next_task_is_admitted
run "a waiting step's activity names what it waits for" \
    -p mason-daemon --test scheduling a_step_waits_for_a_held_class_and_says_why_then_lands
run "the update self-test, with its reasons" \
    -p mason-daemon --test update

echo "s60 OK"
