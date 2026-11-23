#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Stage 1027 exit test: DiskSectionTests, RowRedrawTests and MonitorCoreTests pass.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
if [ -n "${MASON_SKIP_SWIFT:-}" ] || [ "$(uname)" != Darwin ] || ! command -v swift >/dev/null; then
  echo "s1027 skipped: needs macOS and Swift"; exit 0
fi
log=$(mktemp /tmp/mason-s1027.XXXXXX)
trap 'rm -f "$log"' EXIT
if ! (cd clients/swift && swift test --filter 'DiskSectionTests|RowRedrawTests|MonitorCoreTests' >"$log" 2>&1); then
  tail -30 "$log" >&2; echo "FAIL: swift test exited non-zero" >&2; exit 1
fi
grep "MasonMonitorPackageTests.xctest" "$log" | grep -q "with 0 failures"
echo "s1027 OK"
