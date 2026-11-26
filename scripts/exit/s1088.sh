#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1088 exit test: a CI run MASON stops waiting on without a verdict is cancelled
# before its branch is deleted. Real daemons on a local bare repository with the
# command CI as the fake, which writes down every cancel; no network.
set -euo pipefail
cd "$(dirname "$0")/../.."

run() {
    local name=$1
    echo "== $name"
    out=$(cargo test -q -p mason-daemon --test it "$name" -- --exact 2>&1) || { echo "$out" >&2; echo "FAILED: $name" >&2; exit 1; }
    echo "$out" | grep -qE '^test result: ok\. 1 passed' || { echo "$out" >&2; echo "FAILED (did not run): $name" >&2; exit 1; }
}

for t in \
    a_direct_ci_batch_whose_ci_never_finishes_cancels_its_run_once \
    land_now_on_a_batch_waiting_for_ci_cancels_its_run \
    a_batch_a_dead_daemon_left_has_its_run_cancelled_before_its_ref_goes \
    a_green_run_cancels_nothing \
    a_cancel_that_fails_leaves_the_batchs_outcome_unchanged \
    a_merged_pull_request_cancels_nothing \
    a_red_pull_request_has_its_other_checks_cancelled \
    a_debt_ci_run_that_times_out_is_cancelled
do
    run "ci_cancel::$t"
done

echo "s1088 OK"
