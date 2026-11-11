#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s123 exit test: the state icons are wired into every place the ticket names, on both desktop
# clients and the phone, as decorative images sized by the text-zoom tokens. This is a source
# check; it does not build or draw the apps (the Swift and Compose builds run in the test suite).
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
c=clients
need() { grep -q -- "$2" "$1" || { echo "FAILED: $1 lacks: $2" >&2; exit 1; }; }
for f in Components TopBar Sidebar Pages; do
  need "$c/swift/Sources/MonitorUI/$f.swift" 'StateIcon('
  need "$c/compose/app/src/main/kotlin/mason/monitor/ui/$f.kt" 'StateIcon('
done
need "$c/swift/Sources/MonitorPhone/Phone.swift" 'PhoneStateIcon('
need "$c/swift/Sources/MonitorUI/Components.swift" 'accessibilityHidden(true)'
need "$c/compose/app/src/main/kotlin/mason/monitor/ui/Mascot.kt" 'contentDescription = null'
need "$c/swift/Sources/MonitorUI/Components.swift" 'overlay(alignment: .topLeading)'
need "$c/compose/app/src/main/kotlin/mason/monitor/ui/Components.kt" 'layout(0, 0)'
need "$c/features.toml" 'feature.mascot-state-icons'
# No literal icon sizes in the views: they come from the tokens.
if grep -nE 'StateIcon.*(20|24)\b' "$c"/swift/Sources/MonitorUI/*.swift "$c"/compose/app/src/main/kotlin/mason/monitor/ui/*.kt; then
  echo "FAILED: literal icon size in a view" >&2; exit 1
fi
# The shared item files, which the conformance run compares, are untouched.
if git diff --name-only origin/develop... 2>/dev/null | grep -E 'MonitorCore/Items.swift|core/.*/Items.kt'; then
  echo "FAILED: Items.swift or Items.kt changed" >&2; exit 1
fi
echo "s123: ok"
