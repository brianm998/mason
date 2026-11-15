#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s435 exit test: both monitors have the section pages, the '>' links and the tile rule.
# The behaviour is tested in MonitorCoreTests (Swift) and MonitorCoreTest (Kotlin), run by
# swift test / gradle; this checks that the pieces are present in both clients.
set -euo pipefail
cd "$(dirname "$0")/../.."
need() { grep -q "$2" "$1" || { echo "s435: missing '$2' in $1"; exit 1; }; }
need clients/swift/Sources/MonitorCore/Navigation.swift 'mason://section/'
need clients/compose/core/src/main/kotlin/mason/monitor/Navigation.kt 'mason://section/'
need clients/swift/Sources/MonitorCore/Model.swift 'tileDoubleClicked'
need clients/compose/core/src/main/kotlin/mason/monitor/Model.kt 'tileDoubleClicked'
need clients/swift/Sources/MonitorUI/Pages.swift 'chevron.right'
need clients/swift/Sources/MonitorUI/Sidebar.swift 'section-open:'
need clients/compose/app/src/main/kotlin/mason/monitor/ui/Pages.kt 'section-open:'
need clients/compose/app/src/main/kotlin/mason/monitor/ui/Sidebar.kt 'section-open:'
need clients/swift/Tests/MonitorCoreTests/MonitorCoreTests.swift 'testSectionUrlsRoundTripToTheirPages'
need clients/compose/core/src/test/kotlin/mason/monitor/MonitorCoreTest.kt 'sectionUrlsRoundTripToTheirPages'
echo "s435: ok"
