#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s464 exit test: the CPU-gate finding counts time held under a CPU cause, not the wait's age.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
cargo test -q -p mason-scheduler cpu_hold
if grep -q 'contains("cpu")' crates/daemon/src/health.rs; then echo "FAIL: text match remains" >&2; exit 1; fi
echo "s464: all checks passed"
