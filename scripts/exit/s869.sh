#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s869 exit test (#818): every remote run asks the broker for runner units before it is pushed.
set -euo pipefail
cd "$(dirname "$0")/../.."
for t in \
    a_check_bound_for_github_waits_in_the_broker_until_a_unit_is_free \
    a_run_that_falls_back_gives_its_runner_units_back_before_taking_local_slots \
    a_check_asking_again_after_a_restart_takes_over_its_adopted_hold_once \
    a_suite_holds_one_gha_unit_per_job_and_a_macos_runner_adds_gha_macos \
    a_hold_takes_runner_units_for_a_github_suite_and_local_slots_for_the_rest; do
    echo "== $t"
    cargo test -q -p mason-daemon --lib "$t"
done
echo "s869: ok"
