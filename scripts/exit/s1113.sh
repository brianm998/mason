#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1113 exit test (decision 1102): landing no longer writes README.md. The two status scripts and
# the daemon's regeneration are gone; the numbers prompt says to keep a change out of README.md;
# the landing tests that move the base after assembly still pass.
set -euo pipefail
cd "$(dirname "$0")/../.."

grep -q '^Summary: ' docs/stages/s1113.md
for f in scripts/gen-status.sh scripts/check-status.sh; do
  [ ! -e "$f" ] || { echo "s1113: $f still exists" >&2; exit 1; }
done
if grep -rn 'regenerate_status' crates/; then
  echo "s1113: crates/ still has regenerate_status" >&2; exit 1
fi
if [ -z "${MASON_TESTS_ALREADY_RAN:-}" ]; then
  cargo test -q -p mason-daemon --lib -- numbers::
  cargo test -q -p mason-daemon --test it -- \
    landing::the_retest_after_a_moved_base landing::the_retest_builds_holding_compile \
    landing::a_batch_whose_testing_was_skipped_is_rebuilt
fi
echo "s1113 ok"
