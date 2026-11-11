#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s121 exit test: the bundle script makes an icon and names it in the plist; Compose and iOS have it.
set -euo pipefail
cd "$(dirname "$0")/../.."
grep -q 'CFBundleIconFile' scripts/bundle-monitor.sh
grep -q 'iconutil' scripts/bundle-monitor.sh
grep -q 'icon = ' clients/compose/app/src/main/kotlin/mason/monitor/Main.kt
test -f clients/swift/ios/MasonIOS/AppIcon-1024.png
plutil -lint clients/swift/ios/MasonIOS/Info.plist
