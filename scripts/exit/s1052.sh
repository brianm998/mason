#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1052 exit test: the Swift monitor has Add Project (File menu and a + under the project picker),
# desktop only, and the Swift tests are green.
set -euo pipefail
cd "$(dirname "$0")/../.."
grep -q 'Add Project…' clients/swift/Sources/MasonMonitor/App.swift
grep -q 'add-project' clients/swift/Sources/MonitorUI/TopBar.swift
test -f clients/swift/Sources/MonitorUI/AddProjectSheet.swift
test -f clients/swift/Sources/MonitorCore/AddProject.swift
# The sheet and the + are macOS-only: the iOS app shares MonitorUI and must show neither.
head -5 clients/swift/Sources/MonitorUI/AddProjectSheet.swift | grep -q '#if os(macOS)'
grep -B3 'add-project"' clients/swift/Sources/MonitorUI/TopBar.swift | grep -q 'canAddProject'
if grep -rq 'add-project\|AddProject' clients/swift/Sources/MonitorPhone clients/swift/Sources/MonitorUI/Phone.swift clients/swift/Sources/MonitorUI/PhoneScreens.swift; then
  echo "phone layout mentions add project" >&2
  exit 1
fi
if [ "$(uname)" = Darwin ] && command -v swift >/dev/null && [ -z "${MASON_SKIP_SWIFT:-}" ]; then
  swift test --package-path clients/swift --filter 'AddProjectTests|AddProjectSnapshotTests'
fi
