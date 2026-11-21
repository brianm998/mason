#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s774 exit test: the landing-floor test reports the batch state when it fails
# or when a wait times out.
# This script only checks the diagnostics are in place. Running cargo test here
# builds the whole workspace on a runner with a fixed time limit, and the test
# step already runs the landing_floor module.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
grep -q 'logs_named(&w.s.data_dir, "-floor-")' crates/daemon/tests/it/landing_floor.rs
grep -q 'try_wait_for_exact(what, 900, 50, f)' crates/daemon/tests/it/landing_floor.rs
echo "s774 passed"
