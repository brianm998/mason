#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# A landing batch holds this machine's slots only for the suites that run on this machine; a
# suite bound for GitHub takes its slots, as the batch, only if it comes back here (s943, 0936).
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
cargo test -q -p mason-daemon --lib -- --exact \
  suites::batch_cover_tests::a_batch_cover_holds_only_what_the_lease_holds
cargo test -q -p mason-daemon --test it -- --exact \
  batch_remote_hold::a_batch_whose_only_suite_runs_on_github_holds_no_local_slot_while_it_is_out \
  batch_remote_hold::an_exclusive_request_is_granted_while_a_batch_is_out_on_github \
  batch_remote_hold::a_batch_with_a_local_and_a_remote_suite_holds_only_the_local_suites_slots \
  fallback_lease::a_batch_suite_falling_back_here_takes_its_slots_ranked_as_the_batch
# Part 2: Land now stops the fast tier and the new-tests check (they are owed after the merge);
# without Land now the new-tests check still runs.
cargo test -q -p mason-daemon --test landing_new_tests -- --exact \
  land_now_does_not_run_the_new_tests_check_and_owes_it_after_the_merge \
  a_skipped_task_whose_new_test_fails_is_parked_and_the_others_land
echo "s943: all checks passed"
