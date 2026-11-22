#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# A run GitHub has not picked up comes back to this machine only when it has room for it (s924, 0917).
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
cargo test -q -p mason-daemon --lib -- --exact \
  place::tests::room_needs_no_hold_and_a_free_unit_of_each_slot \
  remote::tests::an_unpicked_favor_remote_run_comes_back_early_when_this_machine_has_room \
  remote::tests::a_run_past_the_pickup_timeout_waits_on_github_until_this_machine_has_room \
  remote::tests::a_run_held_back_for_room_that_gets_a_runner_runs_on_github \
  remote::tests::a_run_with_no_room_function_falls_back_at_the_pickup_timeout \
  remote::tests::no_runner_within_the_pickup_timeout_is_a_wait_charged_to_no_task
cargo test -q -p mason-daemon --test it -- --exact \
  fallback_lease::room_for_a_fallback_reads_the_broker_and_a_covered_run_is_not_blocked_by_its_own_hold
echo "s924: all checks passed"
