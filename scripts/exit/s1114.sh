#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1114 exit test: the round that uses up a step's last chance keeps its feedback. The first case
# replays task #1112: two review send-backs with different findings under max_rounds = 2, the task
# fails, and the second findings are in the history, in the diagnosis prompt and in the develop
# prompt after an owner retry.
set -euo pipefail
cd "$(dirname "$0")/../.."

grep -q '^Summary: ' docs/stages/s1114.md

# One cargo run, so the cases wait for their test worlds side by side and not one after another.
out=$(cargo test -p mason-daemon --test it -- round_feedback:: escalate:: 2>&1) ||
    { echo "$out" >&2; echo "FAILED: s1114 tests" >&2; exit 1; }
echo "$out" | grep -E '^test result' | tail -1

# A filter that matches nothing passes: every case here has to have run.
for t in \
    round_feedback::the_last_reviews_findings_reach_the_history_the_diagnosis_and_the_retried_develop_round \
    round_feedback::a_test_step_red_to_the_last_round_keeps_its_output_for_the_diagnosis \
    round_feedback::a_retry_from_review_that_approves_removes_the_old_findings \
    round_feedback::an_owner_rejection_with_a_way_back_is_in_the_history_and_so_is_the_one_that_fails \
    round_feedback::an_owner_rejection_with_no_way_back_is_in_the_history \
    round_feedback::a_failure_with_no_feedback_leaves_the_earlier_files_alone \
    escalate::the_last_round_runs_on_the_escalation_model_and_earlier_rounds_do_not \
    escalate::an_empty_escalate_model_turns_it_off
do
    echo "$out" | grep -qE "^test $t \.\.\. ok" || { echo "FAILED: $t did not pass" >&2; exit 1; }
done

echo "s1114 OK"
