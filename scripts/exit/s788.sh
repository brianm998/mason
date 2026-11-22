#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s788 exit test: the batch state, severity and typical-time rules, and the client grouping.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
cargo test -q -p mason-daemon --lib batch_state_tests
if [ -z "${MASON_SKIP_SWIFT:-}" ] && command -v swift >/dev/null; then
  (cd clients/swift && swift test --filter LandingItemsTests)
fi
