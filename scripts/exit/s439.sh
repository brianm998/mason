#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s439 exit test: the Swift top bar's minimum width fits the 1000 pt window minimum.
set -euo pipefail
cd "$(dirname "$0")/../../clients/swift"
swift test --filter TopBarFitTests
