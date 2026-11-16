#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s439 exit test: the Swift top bar's minimum width fits the 1000 pt window minimum.
set -euo pipefail
if [ -n "${MASON_SKIP_SWIFT:-}" ] || [ "$(uname)" != Darwin ] || ! command -v swift >/dev/null; then
  echo "s439 skipped: needs macOS and Swift (not run by CI's Linux exit-scripts job)"; exit 0
fi
cd "$(dirname "$0")/../../clients/swift"
swift test --filter TopBarFitTests
