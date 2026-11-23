#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s979 exit test: the Banner has a detail field, the daemon keeps banner text to one short line,
# and both monitors draw one line per row with a "?" button that opens the details.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
grep -q 'string detail = 7;' schemas/mason.proto
grep -q 'fn one_line(b: &mut Banner)' crates/daemon/src/views.rs
grep -q 'BANNER_TEXT_MAX' crates/daemon/src/views.rs
grep -q 'lineLimit(1)' clients/swift/Sources/MonitorUI/TopBar.swift
grep -q 'accessibilityLabel("Details")' clients/swift/Sources/MonitorUI/TopBar.swift
grep -q 'DetailsButton' clients/compose/app/src/main/kotlin/mason/monitor/ui/TopBar.kt
grep -q 'feature.notification-details' clients/features.toml
echo "s979 passed"
