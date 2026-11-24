#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s664 exit test: the skipped-batch fast-tier test passes 10 runs in a row (#650).
set -euo pipefail
cd "$(dirname "$0")/../.."

run10() {
    local crate=$1 file=$2 name=$3 i build bin
    build=$(cargo test -p "$crate" --test "$file" --no-run --message-format=json 2>&1) \
        || { echo "$build" >&2; echo "FAILED: build of $file" >&2; exit 1; }
    bin=$(echo "$build" | grep -oE "\"executable\":\"[^\"]*/deps/${file}-[0-9a-f]+\"" | tail -1 | cut -d'"' -f4 || true)
    [ -n "$bin" ] && [ -x "$bin" ] || { echo "$build" >&2; echo "no test binary for $file" >&2; exit 1; }
    for i in 1 2 3 4 5 6 7 8 9 10; do
        echo "== $name ($i of 10)"
        out=$("$bin" --exact "$name" 2>&1) \
            || { echo "$out" >&2; echo "FAILED: $name on run $i" >&2; exit 1; }
        echo "$out" | grep -E '^test result' | tail -1
    done
}

run10 mason-daemon landing_fast_tier a_skipped_batch_whose_merge_breaks_a_unit_test_is_parked_naming_that_test

echo "s664 OK"
