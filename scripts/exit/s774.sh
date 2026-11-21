#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s774 exit test: the landing-floor test reports the batch state when it fails.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
grep -q "fast tier: {}" crates/daemon/tests/it/landing_floor.rs
for _ in 1 2 3 4 5 6 7 8 9 10; do
  cargo test -q -p mason-daemon --test it landing_floor
done
echo "s774 passed"
