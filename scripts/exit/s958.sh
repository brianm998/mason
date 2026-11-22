#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s958 exit test: the monitor opts out of App Nap in code and in the bundle.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
grep -q 'latencyCritical' clients/swift/Sources/MonitorCore/AppActivity.swift
grep -q 'activity.hold()' clients/swift/Sources/MasonMonitor/App.swift
grep -q 'NSAppSleepDisabled' scripts/bundle-monitor.sh
echo "s958 passed"
