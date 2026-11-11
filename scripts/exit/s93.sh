#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s93 exit test: run_check runs under a step host. A real masond is killed with kill -9 while
# the scripted agent's run_check is running and restarted: the command ran once, the step
# finishes, the task lands, and no mason-sh-* file is left in TMPDIR. A second call attaches,
# a hung check ends at its limit, and a different tree runs afresh.
set -euo pipefail
cd "$(dirname "$0")/../.."

run() {
    local what=$1; shift
    echo "== $what"
    out=$(cargo test -q "$@" 2>&1) || { echo "$out" >&2; echo "FAILED: $what" >&2; exit 1; }
    echo "$out" | grep -E '^test result' | tail -1
}

run "kill -9 during run_check: the command ran once, the task lands, no temp files left" \
    -p mason-daemon --test hosted_check kill_9_while_run_check_runs_the_command_once_and_the_task_lands
run "a second call attaches; a different tree runs afresh" \
    -p mason-daemon --test hosted_check a_second_call_attaches_and_a_different_tree_runs_afresh
run "a hung check ends at the limit" \
    -p mason-daemon --test hosted_check a_hung_check_ends_at_the_limit
run "the host id suffix is understood by the sweep" \
    -p mason-daemon --lib sweep

echo "s93 OK"
