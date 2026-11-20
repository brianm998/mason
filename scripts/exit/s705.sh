#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s705 exit test: a draining update stops a red batch between runs, and the next batch over
# the same base and task heads says it reused the earlier run results.
set -uo pipefail
cd "$(dirname "$0")/../.."
cargo test -q -p mason-store a_batch_run_is_found_again_only_for_the_same_base_and_task_heads || exit 1
# The test asserts the batch note carries the reused-results line.
grep -q 'reused 1 run result(s) from an earlier batch' crates/daemon/tests/it/landing_yield.rs || exit 1
grep -q 'reused {reused} run result(s) from an earlier batch' crates/daemon/src/landing.rs || exit 1
cargo test -q -p mason-daemon --test it a_draining_update_stops_a_red_batch_between_runs_and_the_next_batch_reuses_its_results
