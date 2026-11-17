#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s557 exit test: the monitor builds without the defaultApprovalPriority isolation warning.
set -uo pipefail
cd "$(dirname "$0")/../../clients/swift"
if [ -n "${MASON_SKIP_SWIFT:-}" ] || [ "$(uname)" != Darwin ] || ! command -v swift >/dev/null; then
  echo "s557 skipped: needs macOS and Swift (and is not run by the Rust job)"; exit 0
fi
out=$(swift build 2>&1) || { echo "$out"; exit 1; }
if echo "$out" | grep -q 'nonisolated context'; then echo "$out" | grep 'nonisolated context'; exit 1; fi
swift test 2>&1 | tail -3
