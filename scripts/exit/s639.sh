#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s639 exit test: the open_session tests pass 5 runs in a row.
set -euo pipefail
cd "$(dirname "$0")/../.."

for i in 1 2 3 4 5; do
    echo "== run $i of 5"
    out=$(cargo test -q -p mason-pipeline --bin mason open_session_tests 2>&1) \
        || { echo "$out" >&2; echo "FAILED on run $i" >&2; exit 1; }
    echo "$out" | grep -E '^test result' | tail -2
done

echo "s639 OK"
