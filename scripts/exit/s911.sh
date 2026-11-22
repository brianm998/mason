#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# A suite of a landing batch that falls back from GitHub runs under the batch's own hold of this
# machine's slots instead of queueing for them again behind an exclusive request (s911, 0904).
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
cargo test -q -p mason-daemon --test it -- --exact \
  fallback_lease::a_covered_fallback_runs_under_the_batchs_hold_beside_a_waiting_exclusive_request \
  fallback_lease::a_fallback_run_waits_for_the_slot_it_would_hold_here_and_gets_it_when_freed
echo "s911: all checks passed"
