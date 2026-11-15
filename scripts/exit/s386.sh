#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s386 exit test: a suite with narrow keys runs only the tests a change needs in a task's test step.
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test -q -p mason-testing --lib -- narrow::tests
cargo test -q -p mason-definition --test testing_tables -- the_narrow_keys_parse_into_one_narrow_and_their_mistakes_are_named
cargo test -q -p mason-daemon --test testing -- a_task_test_step_runs_only_the_tests_its_change_needs_and_proves_nothing_for_the_tree
echo "s386 OK"
