#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s160 exit test: Open session goes through the session check in both monitors, never straight
# to the clipboard, and the CLI is named by full path. (No terminal is started: that is s160-manual.)
set -euo pipefail
cd "$(dirname "$0")/../.."
if grep -n 'open_session.*\(copyToPasteboard\|copyToClipboard\)' clients/swift/Sources/MonitorUI/Components.swift clients/compose/app/src/main/kotlin/mason/monitor/ui/Components.kt; then
  echo "s160: Open session still only copies" >&2
  exit 1
fi
grep -q 'model.openSession(row.id)' clients/swift/Sources/MonitorUI/Components.swift
grep -q 'model.openSession(row.id)' clients/compose/app/src/main/kotlin/mason/monitor/ui/Components.kt
grep -q '/tasks/\\(task)/session' clients/swift/Sources/MonitorCore/Model.kt 2>/dev/null || grep -q '/tasks/\\(task)/session' clients/swift/Sources/MonitorCore/Model.swift
grep -q '.config/mason/bin/mason' clients/swift/Sources/MonitorCore/OpenSession.swift
grep -q '.config/mason/bin/mason' clients/compose/core/src/main/kotlin/mason/monitor/OpenSession.kt
echo "s160: ok"
