#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s645 exit test: the offscreen-host Swift tests pass.
set -euo pipefail
if [ -n "${MASON_SKIP_SWIFT:-}" ]; then echo "s645 skipped: MASON_SKIP_SWIFT is set (CI's Rust job)"; exit 0; fi
cd "$(dirname "$0")/../../clients/swift"
swift test --filter 'RowRedrawTests|LazySidebarTests|FollowOnlyWhatTheyShowTests'
echo "s645 OK"
