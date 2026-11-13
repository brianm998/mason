#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s172 exit test: a real daemon with the scripted agent. A task whose test command writes a file
# into $MASON_TASK_CACHE sees the directory while it runs; after the task lands, or is canceled,
# the directory is gone; the sweep removes a finished task's leftover and not a running task's.
set -euo pipefail
cd "$(dirname "$0")/../.."

run() {
    local what=$1; shift
    echo "== $what"
    out=$(cargo test -q "$@" 2>&1) || { echo "$out" >&2; echo "FAILED: $what" >&2; exit 1; }
    echo "$out" | grep -E '^test result' | tail -1
}

run "a step sees the directory; gone after landing" -p mason-daemon --test taskcache_own a_step_sees_the_directory_and_it_is_gone_after_landing
run "gone after cancel" -p mason-daemon --test taskcache_own the_directory_is_gone_after_cancel
run "the sweep removes a leftover" -p mason-daemon --test taskcache_own the_sweep_removes_a_finished_tasks_leftover
run "the sweep leaves a running task's" -p mason-daemon --test taskcache_own the_sweep_leaves_a_running_tasks_directory
echo "s172 OK"
