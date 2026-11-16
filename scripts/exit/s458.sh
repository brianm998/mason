#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s458 exit test: the iPad layout fit tests pass and the iOS app builds. macOS only.
set -euo pipefail
cd "$(dirname "$0")/../.."
[ "$(uname)" = Darwin ] && [ -z "${MASON_SKIP_SWIFT:-}" ] || { echo "s458 SKIP: needs macOS for the Swift package"; exit 0; }
grep -q 'iPadTextScale = 0.9' clients/swift/Sources/MonitorCore/PlatformFeatures.swift
grep -q 'features: .iPad' clients/swift/ios/MasonIOS/MasonIOSApp.swift
(cd clients/swift && swift test --filter IPadLayoutTests)
make -C clients/swift ios
echo "s458 OK"
