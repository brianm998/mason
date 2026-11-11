#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s121 exit test: the bundle script makes an icon and names it in the plist; Compose and iOS have it.
set -euo pipefail
cd "$(dirname "$0")/../.."
grep -q 'CFBundleIconFile' scripts/bundle-monitor.sh
grep -q 'iconutil' scripts/bundle-monitor.sh
grep -q 'icon = ' clients/compose/app/src/main/kotlin/mason/monitor/Main.kt
test -f clients/swift/ios/MasonIOS/AppIcon-1024.png
# plutil is macOS's; elsewhere the plist is checked as XML.
if command -v plutil >/dev/null; then
  plutil -lint clients/swift/ios/MasonIOS/Info.plist
else
  python3 -I -c 'import plistlib,sys; plistlib.load(open(sys.argv[1],"rb"))' clients/swift/ios/MasonIOS/Info.plist
fi
