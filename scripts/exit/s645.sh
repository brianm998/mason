#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s645 exit test: the offscreen-host Swift tests pass.
set -euo pipefail
if [ -n "${MASON_SKIP_SWIFT:-}" ] || [ "$(uname)" != Darwin ] || ! command -v swift >/dev/null; then
  echo "s645 skipped: needs macOS and Swift (CI's monitor-swift job runs the whole suite)"
  exit 0
fi
cd "$(dirname "$0")/../../clients/swift"
swift test --filter 'RowRedrawTests|LazySidebarTests|FollowOnlyWhatTheyShowTests'
echo "s645 OK"
