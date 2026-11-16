#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s466 exit test: the iOS push registration is wired in. The Swift tests and `make ios` run in the
# test step and by the owner; this checks that the pieces exist. A real push to a phone is the owner's check.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)/clients/swift"
grep -q "aps-environment" ios/MasonIOS/MasonIOS.entitlements
grep -q "registerForRemoteNotifications" ios/MasonIOS/MasonIOSApp.swift
grep -q "/devices/self/push" Sources/MonitorCore/Push.swift
grep -q "testForgetDeletesBeforeClearing" Tests/MonitorCoreTests/PushTests.swift
echo "s466 ok"
