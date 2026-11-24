#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1070 exit test: runner units go landing, check, test step, debt; debts capped (decision 1059).
set -euo pipefail
cd "$(dirname "$0")/../.."

grep -q '^Summary: ' docs/stages/s1070.md
cargo test -q -p mason-scheduler --test runner_order
cargo test -q -p mason-scheduler runner_units_go_landing
cargo test -q -p mason-daemon --lib a_debt_of_eight_shards_runs_as_four
echo "OK s1070"
