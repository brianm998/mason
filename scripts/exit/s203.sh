#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s203 exit test: the top bar stacks its groups in both clients, the daemon pushes both windows,
# and the tests exist. (The Swift and Compose tests run under swift test / gradle.)
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
! grep -q "max_by(|x, y|" crates/daemon/src/usage.rs
grep -q "shortest first" crates/daemon/src/usage.rs
grep -q "testCrowdedTopBarFitsTheMainWindowMinimum" clients/swift/Tests/MonitorUITests/TopBarFitTests.swift
grep -q "testZeroCountIsMutedAndNonZeroIsNot" clients/swift/Tests/MonitorUITests/TopBarFitTests.swift
grep -q "zeroCountIsMutedAndNonZeroIsNot" clients/compose/app/src/test/kotlin/mason/monitor/ui/TopBarTest.kt
grep -q "topBarUsageLines" clients/swift/Sources/MonitorUI/TopBar.swift
grep -q "topBarUsageLines" clients/compose/app/src/main/kotlin/mason/monitor/ui/TopBar.kt
grep -q "Grid(" clients/swift/Sources/MonitorUI/TopBar.swift
cargo test -p mason-daemon --test usage
echo "s203 ok"
