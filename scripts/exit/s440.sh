#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s440 exit test: the top bar fits the main window's minimum width with attention showing.
set -euo pipefail
cd "$(dirname "$0")/../../clients/swift"
swift test --filter TopBarFitTests
echo "s440 OK"
