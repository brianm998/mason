#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s440 exit test: the top bar fits the main window's minimum width with attention showing.
set -euo pipefail
if [ -n "${MASON_SKIP_SWIFT:-}" ] || [ "$(uname)" != Darwin ] || ! command -v swift >/dev/null; then
  echo "s440 skipped: needs macOS and Swift (not run by CI's Linux exit-scripts job)"; exit 0
fi
cd "$(dirname "$0")/../../clients/swift"
swift test --filter TopBarFitTests
echo "s440 OK"
