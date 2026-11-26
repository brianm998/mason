#!/bin/bash
# SPDX-License-Identifier: GPL-3.0-or-later
set -u
cd "$(dirname "$0")/../../clients/swift" || exit 1
f=Tests/MonitorCoreTests/WatchdogTests.swift
n=$(grep -c '\.start(' "$f")
if [ "$n" -ne 1 ]; then echo "FAIL: WatchdogTests calls .start( $n times, expected 1"; exit 1; fi
swift test --filter WatchdogTests || { echo "FAIL: WatchdogTests"; exit 1; }
