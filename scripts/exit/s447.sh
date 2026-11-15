#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s447 exit test: the monitor renders offscreen at both iPad landscape sizes, light and dark.
set -euo pipefail
cd "$(dirname "$0")/../../clients/swift"
swift test --filter IPadSnapshotTests
echo "s447 OK"
