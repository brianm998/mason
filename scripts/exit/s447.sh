#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s447 exit test: the monitor renders offscreen at both iPad landscape sizes, light and dark.
set -euo pipefail
if [ -n "${MASON_SKIP_SWIFT:-}" ] || [ "$(uname)" != Darwin ] || ! command -v swift >/dev/null; then
  echo "s447 skipped: needs macOS and Swift (not run by CI's Linux exit-scripts job)"; exit 0
fi
cd "$(dirname "$0")/../../clients/swift"
swift test --filter IPadSnapshotTests
echo "s447 OK"
