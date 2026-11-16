#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s457 exit test: the iOS pairing screen is wired in. The Swift is not built here (that is
# `make ios` and `swift test` on a Mac with Xcode); this checks the pieces are present.
set -euo pipefail
cd "$(dirname "$0")/../.."
p=clients/swift/ios/MasonIOS/Info.plist
grep -q 'NSCameraUsageDescription' "$p"
grep -q '<string>mason</string>' "$p"
grep -q 'DataScannerViewController' clients/swift/Sources/MonitorPhone/PairingViews.swift
grep -q 'PairingScreenState' clients/swift/Tests/MonitorCoreTests/PairingScreenTests.swift
grep -q 'onOpenURL' clients/swift/ios/MasonIOS/MasonIOSApp.swift
echo "s457 ok (Swift not compiled here)"
