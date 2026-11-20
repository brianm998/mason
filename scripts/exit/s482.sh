#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s482 exit test: the hold-across-a-kill test passes 10 runs in a row.
set -euo pipefail
cd "$(dirname "$0")/../.."

run10() {
    local crate=$1 file=$2 name=$3 i
    for i in $(seq 1 "${MASON_EXIT_REPEAT:-10}"); do
        echo "== $name ($i of ${MASON_EXIT_REPEAT:-10})"
        out=$(cargo test -q -p "$crate" --test "$file" "$name" -- --exact 2>&1) \
            || { echo "$out" >&2; echo "FAILED: $name on run $i" >&2; exit 1; }
        echo "$out" | grep -E '^test result' | tail -1
    done
}

run10 mason-daemon holds a_real_daemon_keeps_a_hold_across_a_kill_and_the_cli_holds_and_releases

echo "s482 OK"
