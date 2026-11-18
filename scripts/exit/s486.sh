#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s486 exit test: the restart-limit and live-token tests pass 5 runs in a row.
set -euo pipefail
cd "$(dirname "$0")/../.."

for i in 1 2 3 4 5; do
    echo "== run $i of 5"
    out=$(cargo test -q -p mason-daemon --test it restart_limit:: live_tokens:: \
        -- restart_limit times_out a_running_steps_cell 2>&1) \
        || { echo "$out" >&2; echo "FAILED on run $i" >&2; exit 1; }
    echo "$out" | grep -E '^test result' | tail -2
done

echo "s486 OK"
