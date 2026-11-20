#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Exit test for s726: the landing fast-tier tests are not ignored and the debt payment is cancellable.
set -euo pipefail
cd "$(dirname "$0")/../.."
if grep -n '#\[ignore' crates/daemon/tests/landing_fast_tier.rs; then exit 1; fi
grep -q 'PAY_CANCELLED' crates/daemon/src/suites.rs
