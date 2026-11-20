#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s686 exit test: the skipped-batch landing test waits for teardown before paying, and passes ten runs.
set -euo pipefail
cd "$(dirname "$0")/../.."
f=crates/daemon/tests/landing_fast_tier.rs
a=$(grep -n 'running_batches' "$f" | head -1 | cut -d: -f1)
b=$(grep -n 'suites::pay' "$f" | head -1 | cut -d: -f1)
[ "$a" -lt "$b" ] || { echo "FAILED: pay comes before the teardown wait" >&2; exit 1; }
for i in 1 2 3 4 5 6 7 8 9 10; do
    echo "== run $i"
    cargo test -q -p mason-daemon --test landing_fast_tier a_skipped_batch_whose_unit_tests_pass_lands || { echo "FAILED: run $i" >&2; exit 1; }
done
