#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s170 exit test: four daemon tests that failed on a busy machine now pass every time.
# Each runs 10 times in a row and every run must pass.
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

run10 mason-daemon it crash::killing_the_daemon_at_every_effect_boundary_loses_nothing_and_repeats_nothing
run10 mason-daemon it sweep::a_healthy_daemon_files_no_bugs_across_many_passes
run10 mason-daemon holds a_real_daemon_keeps_a_hold_across_a_kill_and_the_cli_holds_and_releases
run10 mason-daemon it protocol::when_a_step_ends_the_next_task_is_the_next_one_admitted

echo "s170 OK"
