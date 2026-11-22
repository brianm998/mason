#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# The two suites_schedule tests that were red at ac5b0ff796 pass (s894).
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
cargo test -q -p mason-daemon --test it a_due_schedule_with_remote_off_runs_each_listed_suite_once
cargo test -q -p mason-daemon --test it the_default_runs_the_test_steps_suites_and_the_run_waits
echo "s894 ok"
