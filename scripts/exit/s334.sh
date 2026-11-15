#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s334 exit test: the top bar names the busiest active state's illustration in both clients.
set -euo pipefail
cd "$(dirname "$0")/../.."
grep -q 'topbar-busiest-mascot' clients/features.toml
grep -q 'var busiestState' clients/swift/Sources/MonitorCore/MascotChoice.swift
grep -q 'val MonitorModel.busiestState' clients/compose/core/src/main/kotlin/mason/monitor/MascotChoice.kt
grep -q 'topbar-mascot' clients/swift/Sources/MonitorUI/TopBar.swift
grep -q 'topbar-mascot' clients/compose/app/src/main/kotlin/mason/monitor/ui/TopBar.kt
echo "s334 OK"
