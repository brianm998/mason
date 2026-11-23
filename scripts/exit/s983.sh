#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Stage 983 exit test: long blocker lists wrap (sidebar row, task page).
# Needs macOS and Swift for the tests; skipped elsewhere, and when MASON_SKIP_SWIFT is set.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
# No HStack may directly contain a ForEach of TaskLink in the two files.
for f in Sidebar Pages; do
  if tr '\n' ' ' <"clients/swift/Sources/MonitorUI/$f.swift" | grep -Eq 'HStack[^{]*\{[^{}]*ForEach\([^{}]*\{[^{}]*TaskLink'; then
    echo "FAIL: $f.swift has an HStack over a ForEach of TaskLink" >&2; exit 1
  fi
done
if [ -n "${MASON_SKIP_SWIFT:-}" ] || [ "$(uname)" != Darwin ] || ! command -v swift >/dev/null; then
  echo "s983: swift tests skipped: needs macOS and Swift"; exit 0
fi
log=$(mktemp /tmp/mason-s983.XXXXXX)
trap 'rm -f "$log"' EXIT
if ! (cd clients/swift && swift test --filter 'BlockerLineTests|QueueWaitTests' >"$log" 2>&1); then
  tail -30 "$log" >&2; echo "FAIL: swift test exited non-zero" >&2; exit 1
fi
echo "s983: all checks passed"
