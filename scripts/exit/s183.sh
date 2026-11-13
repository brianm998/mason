#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Stage 183 exit test: the batch union is verified locally before CI.
#  1. Two tasks, each green alone and red together on the batch tier's suite: the batch is red,
#     neither lands, nothing is pushed to mason/ci/*, no CI run is spent, both are told.
#  2. star's example landing config has a batch tier of the cheap static suites, and `mason doctor`
#     warns when a CI or PR mode has none.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
cargo build -q --workspace --bins

run() {  # PACKAGE TEST-NAME [TEST-FILE]
  # CI runs `cargo test --workspace` first, which runs every one of these tests.
  [ -z "${MASON_TESTS_ALREADY_RAN:-}" ] || return 0
  local args=(-p "$1")
  if [ -n "${3:-}" ]; then args+=(--test "$3"); fi
  cargo test -q "${args[@]}" -- --exact "$2" 2>&1 | grep -q 'test result: ok. 1 passed' \
    || { echo "FAILED: $2" >&2; cargo test "${args[@]}" -- --exact "$2" 2>&1 | tail -30 >&2; exit 1; }
}
echo "== 1. a union-only red is caught before CI"
run mason-daemon direct_ci_batch_red_only_on_the_union_is_not_pushed_and_both_tasks_are_told landing_modes
echo "== 2. star's batch tier and the doctor warning"
run mason-pipeline doctor::tests::star_example_has_a_cheap_batch_tier_the_impact_map_narrows
run mason-pipeline doctor::tests::doctor_warns_when_a_ci_or_pr_mode_has_no_batch_tier
echo "s183 OK"
