#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s438: the monitor update test parses the request body.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
cd clients/swift && swift test --filter MonitorCoreTests.MonitorCoreTests/testTheMonitorButtonFollowsTheBuildAndInstallsItOnce
echo "s438 OK"
