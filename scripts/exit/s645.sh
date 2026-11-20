#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s645 exit test: the offscreen-host Swift tests pass.
set -euo pipefail
cd "$(dirname "$0")/../../clients/swift"
if [ -z "${MASON_SKIP_SWIFT:-}" ] && [ "$(uname)" = Darwin ] && command -v swift >/dev/null; then
  swift test --filter 'RowRedrawTests|LazySidebarTests|FollowOnlyWhatTheyShowTests'
else
  echo "s645 skipped: needs macOS and Swift (CI's monitor-swift job runs the whole suite)"
fi
echo "s645 OK"
