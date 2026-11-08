#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s61 exit test: a test step folds in the base, reruns a red once when asked, and tells a
# step what it holds.
set -euo pipefail
cd "$(dirname "$0")/../.."

run() {
    local what=$1; shift
    echo "== $what"
    out=$(cargo test -q "$@" 2>&1) || { echo "$out" >&2; echo "FAILED: $what" >&2; exit 1; }
    echo "$out" | grep -E '^test result' | tail -1
}

run "a red that is green when run again is a flake, not a red round" \
    -p mason-daemon --test e2e a_red_test_that_is_green_when_run_again_is_a_flake_not_a_red_round
run "red every time is still red, with reruns" \
    -p mason-daemon --test e2e a_test_that_is_red_every_time_is_still_red_with_reruns
run "the test step folds in the base" \
    -p mason-daemon --test e2e the_test_step_folds_in_the_base_so_a_fix_that_landed_meanwhile_counts
run "a step is told what it holds" \
    -p mason-daemon --test scheduling a_step_is_told_what_it_holds_so_its_command_need_not_queue_for_it
run "an explicit reload is not raced by the landing job's own" \
    -p mason-daemon --test definitions an_invalid_definition_edit_is_refused_and_the_old_one_keeps_running

echo "s61 OK"
