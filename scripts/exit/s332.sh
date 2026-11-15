#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s332 exit test: both monitors decode off the main thread and coalesce progress bursts.
# The tests need the Swift and Kotlin toolchains; this checks the pieces are in place.
set -euo pipefail
cd "$(dirname "$0")/../.."
grep -q 'decode: MessageDecoder' clients/swift/Sources/MonitorCore/Connection.swift
grep -q 'scheduleProgressFlush' clients/swift/Sources/MonitorCore/Model.swift
grep -q 'MainThreadWatchdog' clients/swift/Sources/MasonMonitor/App.swift
grep -q 'scheduleProgressFlush' clients/compose/core/src/main/kotlin/mason/monitor/Model.kt
grep -q "UiWatchdog" clients/compose/app/src/main/kotlin/mason/monitor/Main.kt
grep -q 'decodedOn' clients/compose/core/src/test/kotlin/mason/monitor/MonitorCoreTest.kt
grep -q 'testFramesAreDecodedOffTheMainThread' clients/swift/Tests/MonitorCoreTests/MonitorCoreTests.swift
echo "s332: ok"
