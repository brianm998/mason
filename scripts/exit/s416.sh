#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s416 exit test: a test red in three tasks is made known red only if it is red at the merge base too.
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test -q -p mason-daemon --lib -- exhausted::tests
cargo test -q -p mason-daemon --test testing -- a_test_red_in_three_tasks_but_green_at_the_base_is_not_made_known_red a_baseline_failure_is_excused_but_a_new_failure_is_the_tasks
echo "s416 OK"
