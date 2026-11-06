#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Stage 14 exit test: the landing engine (direct and local-only modes).
#  1. A batch of five tasks, one of which conflicts with another and one of
#     which is the culprit of a red: the other three land, the conflict and the
#     culprit are sent back, and nothing red reaches the base.
#  2. Two batches whose files and suites do not overlap run in parallel; one
#     that overlaps waits; the queue's idle time is reported.
#  3. kill -9 of the real masond at each landing phase (formed, merged, red,
#     verified, before and after promote) recovers: the clean task lands exactly
#     once and the culprit never lands.
#  4. The rest: the red ladder, land-now and force, the freeze and its fix task,
#     the configuration (a tier that names CI is refused with a pointer), the
#     integration branch's idempotence, and the monitors' Landing page.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
cargo build -q --workspace --bins

t() {  # PACKAGE TEST-FILE TEST-NAME
  cargo test -q -p "$1" --test "$2" -- --exact "$3" 2>&1 | tail -3 | grep -q 'test result: ok. 1 passed' \
    || { echo "FAILED: $2::$3" >&2; cargo test -p "$1" --test "$2" -- --exact "$3" 2>&1 | tail -25 >&2; exit 1; }
}

echo "== 1. a batch of five: three land, the conflict and the culprit are sent back"
t mason-daemon landing a_batch_of_five_lands_three_ejects_the_conflict_and_parks_the_culprit
echo "   t1, t2, t3 landed once each; t4 conflicted with t3 and went back; t5 broke the suite and went back alone"

echo "== 2. disjoint batches run in parallel, overlapping ones wait"
t mason-daemon landing disjoint_batches_run_in_parallel_and_overlapping_ones_wait
echo "   a second batch ran beside a slow one; a batch sharing its suite did not"

echo "== 3. kill -9 at every landing phase of a batch with a culprit"
t mason-daemon landing_crash killing_the_daemon_at_each_landing_phase_of_a_batch_with_a_culprit_loses_and_repeats_nothing
echo "   batch_formed batch_merged batch_red batch_verified batch_before_promote batch_after_promote land_before_event: each recovered"

echo "== 4. the rest of the engine"
cargo test -q -p mason-landing 2>&1 | grep -E 'test result' | head -5
for name in \
  the_red_ladder_blames_by_impact_before_it_bisects \
  a_flaky_red_passes_on_the_rerun \
  land_now_skips_the_window_and_force_skips_verification_leaving_debts \
  a_red_post_land_tier_freezes_landing_until_the_fix_lands \
  a_tier_that_names_ci_is_refused_with_a_pointer_to_the_ci_mode \
  a_mode_that_needs_a_service_the_project_lacks_is_refused_naming_it \
  landing_with_no_config_is_the_old_direct_landing; do
  t mason-daemon landing "$name"
  echo "   ok  $name"
done
if [ -z "${MASON_TESTS_ALREADY_RAN:-}" ]; then
  cargo test -q -p mason-daemon --test crash 2>&1 | grep -E 'test result'
fi
# The monitors' Landing page: Swift and Compose both show batches, phases, freeze and idle time.
cargo run -q -p mason-conformance -- features clients/features.toml | tail -1
echo "s14 OK"
