#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s672 exit test: the proposed and stuck tiles fill red only while one of their tasks is shown.
set -euo pipefail
cd "$(dirname "$0")/../.."
grep -q 'tileShowsCurrentTask' clients/swift/Sources/MonitorCore/Model.swift
grep -q 'tileShowsCurrentTask' clients/swift/Sources/MonitorUI/Components.swift
grep -q 'tileShowsCurrentTask' clients/compose/core/src/main/kotlin/mason/monitor/Model.kt
grep -q 'tileShowsCurrentTask' clients/compose/app/src/main/kotlin/mason/monitor/ui/Components.kt
echo "s672 OK"
