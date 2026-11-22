#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s521 exit test: every landing batch runs the lint tier, tests skipped or not; a red lint parks the batch.
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test -q -p mason-daemon --lib -- lint_places
# Only the named lint tests, exactly: the substring `lint` selected every test of the `it` binary
# with that word in its path, and the run outlasted its 900 s limit (#880).
cargo test -q -p mason-daemon --test it -- --exact \
  landing::a_batch_that_is_not_lint_clean_is_parked_with_the_lint_in_its_note \
  landing::a_lint_clean_batch_lands \
  landing::a_batch_whose_testing_was_skipped_still_runs_the_lint_check \
  landing::lint_runs_before_the_batch_suite_and_parks_only_the_task_that_touches_the_file \
  landing::a_lint_red_in_a_file_no_task_touches_is_not_blamed_by_file \
  landing::the_lint_tier_runs_here_while_the_switch_says_github \
  landing::a_lint_tier_set_to_github_goes_there_and_holds_no_local_compile_slot_while_queued \
  landing::a_red_lint_run_here_parks_the_task_with_file_and_line
grep -q 'name   = "lint"' .mason/landing.toml
grep -q '^\[suite.lint\]' .mason/mason.toml
echo "s521 OK"
