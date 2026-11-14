#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s264 exit test: skipping the test step ends a waiting run_check and runs nothing.
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo build -q --workspace --bins
cargo test -q -p mason-daemon --test scheduling -- skipping_the_test_step_ends_a_waiting_run_check_and_runs_nothing
echo "s264 OK"
