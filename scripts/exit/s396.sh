#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s396 exit test: a suite with narrow_packages runs only the changed crates in a task's test step.
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test -q -p mason-definition --test testing_tables -- narrow_packages
cargo test -q -p mason-daemon --test testing -- a_task_test_step_runs_only_the_crates_its_change_touches_and_their_dependents
echo "s396 OK"
