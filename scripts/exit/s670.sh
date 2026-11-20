#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s670 exit test: the restart-in-the-middle token test passes 5 runs in a row.
set -euo pipefail
cd "$(dirname "$0")/../.."

for i in 1 2 3 4 5; do
    echo "== run $i of 5"
    out=$(cargo test -q -p mason-daemon --test it live_tokens::a_restart_in_the_middle 2>&1) \
        || { echo "$out" >&2; echo "FAILED on run $i" >&2; exit 1; }
    echo "$out" | grep -E '^test result' | tail -1
done

echo "s670 OK"
