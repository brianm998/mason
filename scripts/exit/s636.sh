#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s636 exit test: the hold-across-a-kill test waits for the rate-limit window and passes MASON_EXIT_REPEAT runs (default 10) in a row.
set -euo pipefail
cd "$(dirname "$0")/../.."

run10() {
    local crate=$1 file=$2 name=$3 i n=${MASON_EXIT_REPEAT:-10}
    for i in $(seq 1 "$n"); do
        echo "== $name ($i of $n)"
        out=$(cargo test -q -p "$crate" --test "$file" "$name" -- --exact 2>&1) \
            || { echo "$out" >&2; echo "FAILED: $name on run $i" >&2; exit 1; }
        echo "$out" | grep -E '^test result' | tail -1
    done
}

run10 mason-daemon holds a_real_daemon_keeps_a_hold_across_a_kill_and_the_cli_holds_and_releases

echo "s636 OK"
