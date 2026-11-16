#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s459 exit test: the iPhone portrait layout (list, then detail). The navigation and layout tests
# pass and the universal iOS app still builds (s446's script skips where Xcode is missing).
set -euo pipefail
if [ -n "${MASON_SKIP_SWIFT:-}" ] || [ "$(uname)" != Darwin ] || ! command -v swift >/dev/null; then
  echo "s459 skipped: needs macOS and Swift (not run by CI's Linux exit-scripts job)"; exit 0
fi
cd "$(dirname "$0")/../../clients/swift"
swift test --filter PhoneNavTests
swift test --filter PhoneLayoutTests
bash ../../scripts/exit/s446.sh
echo "s459 OK"
