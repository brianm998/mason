#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s78 exit test: a real daemon with the scripted agent. A task whose test command writes into its
# task_cache directory lands and the directory is gone; a canceled task's remains; a directory
# planted for an already-landed task is removed by the sweep. (The integration tests start the
# real daemon, real git, real step hosts and the scripted agent.)
set -euo pipefail
cd "$(dirname "$0")/../.."

run() {
    local what=$1; shift
    echo "== $what"
    out=$(cargo test -q "$@" 2>&1) || { echo "$out" >&2; echo "FAILED: $what" >&2; exit 1; }
    echo "$out" | grep -E '^test result' | tail -1
}

run "a landed task's directory is gone" -p mason-daemon --test taskcache the_directory_is_gone_after_landing
run "a canceled task's directory remains" -p mason-daemon --test taskcache a_landed_tasks_directory_is_removed_and_a_canceled_tasks_stays
run "the sweep removes a landed task's leftover" -p mason-daemon --test taskcache the_sweep_removes_a_landed_tasks_leftover
run "the sweep leaves a live task's directory" -p mason-daemon --test taskcache the_sweep_leaves_a_live_tasks_directory
run "each path rule" -p mason-daemon --lib taskcache
echo "s78 OK"
