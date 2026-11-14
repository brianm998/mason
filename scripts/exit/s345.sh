#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s345 exit test: a test step's build suites run side by side.
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test -q -p mason-daemon --test scheduling -- a_steps_build_suites_run_side_by_side
echo "s345 OK"
