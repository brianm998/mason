#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s774 exit test: the landing-floor test reports the batch state when it fails.
# This script only checks the diagnostics are in place. Running cargo test here
# builds the whole workspace on a runner with a fixed time limit, and the test
# step already runs the landing_floor module.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
grep -q "fast tier: {}" crates/daemon/tests/it/landing_floor.rs
echo "s774 passed"
