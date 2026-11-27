#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1145 exit test: a scratch daemon and repository with the scripted agent, the sweep's clock
# passed in. A task canceled with its branch kept loses its worktree after a day and keeps the
# branch; a developing task's idle 1 MB target/ goes and its sources stay; both are reported
# with what they would free before the sweep; a dry run removes nothing.
set -euo pipefail
cd "$(dirname "$0")/../.."

run() {
    local what=$1; shift
    echo "== $what"
    out=$(cargo test -q "$@" 2>&1) || { echo "$out" >&2; echo "FAILED: $what" >&2; exit 1; }
    echo "$out" | grep -q 'test result: ok. [1-9]' || { echo "$out" >&2; echo "FAILED (no test ran): $what" >&2; exit 1; }
    echo "$out" | grep -E '^test result' | tail -1
}

grep -q '^Summary: ' docs/stages/s1145.md
run "the keep rules" -p mason-daemon --lib worktrees::tests
run "reported, removed after a day, branch kept, revived" -p mason-daemon --test it a_canceled_tasks_worktree_goes_after_a_day_and_revive_brings_it_back
run "idle build output reported and removed, sources kept" -p mason-daemon --test it a_waiting_developing_task_loses_idle_build_output_and_keeps_its_sources
run "a dry run only lists" -p mason-daemon --test it low_disk_space_trims_without_waiting_and_a_dry_run_only_lists
run "mason disk's text" -p mason-pipeline disk::tests
echo "s1145 OK"
