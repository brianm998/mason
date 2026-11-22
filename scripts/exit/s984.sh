#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s984 exit test: "failed" is a section page in both monitor clients, and the client tests are green.
set -euo pipefail
cd "$(dirname "$0")/../.."
grep -E 'sectionNames' -A2 clients/swift/Sources/MonitorCore/Navigation.swift | grep -q '"failed"'
grep -E 'sectionNames' -A2 clients/compose/core/src/main/kotlin/mason/monitor/Navigation.kt | grep -q '"failed"'
swift test --package-path clients/swift --filter 'MonitorCoreTests|PhoneNavTests'
(cd clients/compose && ./gradlew :core:test)
