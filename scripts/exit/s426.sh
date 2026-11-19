#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s426 exit test: a skipped check is paid back after its task lands.
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test -q -p mason-daemon --test it -- --exact \
  testing::a_debt_closes_by_itself_when_the_suite_later_goes_green_on_a_base_with_the_task \
  testing::a_debt_is_paid_by_a_green_that_skipped_a_quarantined_test_and_the_test_is_named \
  testing::a_debt_whose_task_added_the_skipped_test_is_not_paid_by_a_green_that_skipped_it \
  testing::a_debt_never_closes_on_a_green_at_a_base_without_the_tasks_commit \
  testing::a_debt_whose_suite_is_still_red_stays_open_and_is_run_once_when_idle \
  testing::a_debt_payment_waits_for_the_test_steps_slot_and_runs_once_it_is_free \
  testing::a_force_skip_stops_a_running_suite_keeps_green_lanes_and_records_a_debt \
  testing::a_skip_ends_the_wait_for_a_build_slot_and_takes_no_slot
echo "s426 OK"
