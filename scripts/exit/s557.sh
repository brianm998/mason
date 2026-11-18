#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s557 exit test: the monitor builds without the defaultApprovalPriority isolation warning.
# The Swift test suite is not run here: it is the monitor-swift job's (s12), and some of its
# SwiftUI hosting tests need a window server.
set -uo pipefail
cd "$(dirname "$0")/../../clients/swift"
if [ -n "${MASON_SKIP_SWIFT:-}" ] || [ "$(uname)" != Darwin ] || ! command -v swift >/dev/null; then
  echo "s557 skipped: needs macOS and Swift (and is not run by the Rust job)"; exit 0
fi
out=$(swift build 2>&1) || { echo "$out"; exit 1; }
if echo "$out" | grep -q 'nonisolated context'; then echo "$out" | grep 'nonisolated context'; exit 1; fi
echo "s557: ok"
