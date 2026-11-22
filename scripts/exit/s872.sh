#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s872 exit test: the sidebar scrolls to its end after a project change, and #518's laziness holds.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
if [ -n "${MASON_SKIP_SWIFT:-}" ] || [ "$(uname)" != Darwin ] || ! command -v swift >/dev/null; then
  echo "s872 skipped: needs macOS and Swift"; exit 0
fi
cd clients/swift
swift test --filter SidebarScrollAfterProjectChangeTests
swift test --filter LazySidebarTests
echo "s872 OK"
