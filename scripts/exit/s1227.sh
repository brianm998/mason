#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1227 exit test: a test step without suites follows placement (decision 1217).
set -euo pipefail
cd "$(dirname "$0")/../.."

grep -q '^Summary: ' docs/stages/s1227.md
cargo test -q -p mason-daemon --test nodes a_test_step_without_suites
cargo test -q -p mason-daemon --test nodes a_red_run_on_the_node_is_judged_red
cargo test -q -p mason-daemon --test nodes a_local_project_runs_its_suiteless_test_step_here
echo "OK s1227"
