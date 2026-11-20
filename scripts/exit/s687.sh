#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s687 exit test: the skipped-batch fast-tier test, alone and with its binary's other tests, 10 runs each.
set -uo pipefail
cd "$(dirname "$0")/../.."
n=${S687_RUNS:-10}
bad=0
out=$(mktemp /tmp/s687.XXXXXX)
trap 'rm -f "$out"' EXIT
t=a_skipped_batch_whose_merge_breaks_a_unit_test_is_parked_naming_that_test
for i in $(seq 1 "$n"); do
  if ! cargo test -q -p mason-daemon --test landing_fast_tier "$t" -- --exact >"$out" 2>&1; then
    bad=$((bad + 1)); echo "single run $i: RED"; grep -E 'reason:|panicked' -A12 "$out" | head -40
  fi
  if ! cargo test -q -p mason-daemon --test landing_fast_tier >"$out" 2>&1; then
    bad=$((bad + 1)); echo "binary run $i: RED"; grep -E 'reason:|panicked' -A12 "$out" | head -40
  fi
done
echo "s687: $bad red out of $((n * 2)) runs"
[ "$bad" -eq 0 ]
