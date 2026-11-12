#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Stage 153 exit test: the CI judge tells infrastructure reds from real ones.
#  1. A candidate red with "runner lost" is rerun once and lands green.
#  2. A candidate red with a test failure is sent back with one CI trigger.
#  3. On recorded GitHub responses: timed_out and a shutdown-signal log are infrastructure,
#     a compile error and a failed XCTest assertion are real, a mix is not rerun, logs are
#     fetched for failed jobs only and without the token at the redirect target.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
cargo build -q --workspace --bins

t() {  # PACKAGE TEST-FILE TEST-NAME
  [ -z "${MASON_TESTS_ALREADY_RAN:-}" ] || return 0
  cargo test -q -p "$1" --test "$2" -- --exact "$3" 2>&1 | tail -3 | grep -q 'test result: ok. 1 passed' \
    || { echo "FAILED: $2::$3" >&2; cargo test -p "$1" --test "$2" -- --exact "$3" 2>&1 | tail -30 >&2; exit 1; }
}

echo "== 1. runner lost: rerun once, lands green"
t mason-daemon ci_infra a_runner_lost_red_is_rerun_once_and_lands_green
echo "== 2. a real red: sent back, one trigger"
t mason-daemon ci_infra a_real_red_is_sent_back_without_a_second_run
echo "== 3. the judge on recorded GitHub responses"
for n in a_timed_out_job_is_infrastructure_without_fetching_its_log \
  a_failure_whose_log_says_the_runner_was_shut_down_is_infrastructure \
  a_compile_error_and_a_failed_xctest_assertion_are_real \
  a_mix_names_each_job_and_only_the_failure_job_has_its_log_fetched \
  a_job_cancelled_with_no_step_run_never_got_a_runner \
  a_log_that_cannot_be_fetched_counts_as_a_real_failure \
  with_the_judge_off_no_log_is_requested_and_no_job_is_reported; do
  t mason-adapters github "$n"
done
echo "s153 OK"
