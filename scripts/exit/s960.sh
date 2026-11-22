#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# The Swift monitor holds redraws and page subscriptions while its window is hidden (s960, 0953).
# The Swift tests run on macOS (on GitHub in the pipeline); here we check that they exist and,
# on a Mac, run them.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
grep -q testHiddenSidebarIsHeldButStillChimes clients/swift/Tests/MonitorCoreTests/VisibilityTests.swift
if [ "$(uname)" = Darwin ]; then
  (cd clients/swift && swift test --filter VisibilityTests)
fi
echo "s960: ok"
