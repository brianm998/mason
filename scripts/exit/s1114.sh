#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1114 exit test: the round that uses up a step's last chance keeps its feedback. The first case
# replays task #1112: two review send-backs with different findings under max_rounds = 2, the task
# fails, and the second findings are in the history, in the diagnosis prompt and in the develop
# prompt after an owner retry.
set -euo pipefail
cd "$(dirname "$0")/../.."

run() {
    local what=$1; shift
    echo "== $what"
    out=$(cargo test -q "$@" 2>&1) || { echo "$out" >&2; echo "FAILED: $what" >&2; exit 1; }
    echo "$out" | grep -E '^test result' | tail -1
    # A filter that matches nothing passes: every case here has to run.
    echo "$out" | grep -qE '^test result: ok\. [1-9]' || { echo "FAILED: $what ran no test" >&2; exit 1; }
}

grep -q '^Summary: ' docs/stages/s1114.md

for t in \
    the_last_reviews_findings_reach_the_history_the_diagnosis_and_the_retried_develop_round \
    a_test_step_red_to_the_last_round_keeps_its_output_for_the_diagnosis \
    a_retry_from_review_that_approves_removes_the_old_findings \
    an_owner_rejection_with_a_way_back_is_in_the_history_and_so_is_the_one_that_fails \
    an_owner_rejection_with_no_way_back_is_in_the_history \
    a_failure_with_no_feedback_leaves_the_earlier_files_alone
do
    run "$t" -p mason-daemon --test it "round_feedback::$t" -- --exact
done

run "escalation: the last round is still escalated" -p mason-daemon --test it escalate::

echo "s1114 OK"
