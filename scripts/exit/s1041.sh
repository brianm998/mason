#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1041 exit test: a recovered test step reuses a run only for the tree and command it ran on.
# A fake GitHub stands in for the real one (the tests restart a daemon by dropping one Site and
# making another over the same record directory).
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
cargo test -q -p mason-daemon --lib -- \
  a_collected_run_is_reused_only_for_the_tree_it_ran_on \
  a_run_in_flight_on_another_tree_is_cancelled_and_started_again \
  a_daemon_that_restarts_collects_the_same_run_and_starts_no_second_one \
  an_environmental_retry_in_flight_at_a_restart_is_reattached_and_collected_once \
  a_replaced_step_attempt_is_cancelled_and_gives_its_unit_back_and_the_newer_one_stays \
  suites::attempts_tests
