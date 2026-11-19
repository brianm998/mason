#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s508 exit test: only the red tests are run on develop; a killed develop run is unknown, not the task's.
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test -q -p mason-daemon --test it testing:: -- a_develop_run_that_is_killed_does_not_count_the_round_against_the_task a_develop_run_of_only_the_failing_tests_that_fails_the_same_test_excuses_it a_baseline_failure_is_excused_but_a_new_failure_is_the_tasks
echo "s508 OK"
