#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s773 exit test: the red-base landing-floor test passes 10 runs in a row (#789).
set -euo pipefail
cd "$(dirname "$0")/../.."

name=landing_floor::a_base_that_does_not_build_holds_the_batch_and_blames_nobody
build=$(cargo test -q -p mason-daemon --test it --no-run --message-format=json 2>&1) \
    || { echo "$build" >&2; echo "FAILED: build" >&2; exit 1; }
# Only the `it` test target: `--no-run` also reports the daemon's own binaries, and the last
# executable cargo prints is not always the test one.
bin=$(printf '%s\n' "$build" | grep '"kind":\["test"\]' | grep '"name":"it"' | grep -o '"executable":"[^"]*"' | tail -1 | sed 's/^"executable":"//; s/"$//')
[ -x "$bin" ] || { echo "$build" >&2; echo "no test binary" >&2; exit 1; }
n=${MASON_EXIT_REPEAT:-10}
for i in $(seq 1 "$n"); do
    echo "== run $i of $n"
    out=$("$bin" --exact "$name" 2>&1) \
        || { echo "$out" >&2; echo "FAILED on run $i" >&2; exit 1; }
    echo "$out" | grep -E '^test result' | tail -1
done
echo "s773 OK"
