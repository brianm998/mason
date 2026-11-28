#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1321 exit test: stuck:long does not count capacity waits (decision 1311).
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test -q -p mason-core -- \
  stuck_long_does_not_count_capacity_waits \
  a_pool_wait_past_the_limit_is_stuck_wait_only \
  the_seventh_check_round_is_stuck_long_whatever_the_waits \
  capacity_waits_are_totalled_from_the_wait_events
cargo test -q -p mason-daemon --test it -- health_facts_total_a_steps_capacity_waits_from_the_log
