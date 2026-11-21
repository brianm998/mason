#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s773 exit test: the red-base landing-floor test passes 10 runs in a row (#789).
set -euo pipefail
cd "$(dirname "$0")/../.."

name=landing_floor::a_base_that_does_not_build_holds_the_batch_and_blames_nobody
build=$(cargo test -q -p mason-daemon --test it --no-run 2>&1) \
    || { echo "$build" >&2; echo "FAILED: build" >&2; exit 1; }
bin=$(echo "$build" | grep -oE "target/[^ )]*/deps/it-[0-9a-f]+" | tail -1)
[ -x "$bin" ] || { echo "$build" >&2; echo "no test binary" >&2; exit 1; }
for i in 1 2 3 4 5 6 7 8 9 10; do
    echo "== run $i of 10"
    out=$("$bin" --exact "$name" 2>&1) \
        || { echo "$out" >&2; echo "FAILED on run $i" >&2; exit 1; }
    echo "$out" | grep -E '^test result' | tail -1
done
echo "s773 OK"
