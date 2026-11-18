#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s553 exit test: a skip with no step name, as the monitor's skip all sends it, ends a base-red
# wait and records the debt against the test step.
set -euo pipefail
cd "$(dirname "$0")/../.."
grep -q 'this project has no test step' crates/daemon/src/suites.rs || { echo "s553: an empty step is not read as the test step" >&2; exit 1; }
cargo test -p mason-daemon --test it a_skip_with_no_step_name
echo "s553: ok"
