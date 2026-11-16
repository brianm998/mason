#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s461 exit test: the iOS icon test reads the asset catalog icon, which is opaque and 1024 px.
set -euo pipefail
cd "$(dirname "$0")/../.."
d=clients/swift/ios/MasonIOS/Assets.xcassets/AppIcon.appiconset
f=$d/AppIcon-1024.png
grep -q 'AppIcon.appiconset/AppIcon-1024.png' clients/swift/Tests/MasonMascotTests/MascotTests.swift
test -f "$f"
grep -q 'AppIcon-1024.png' "$d/Contents.json"
if command -v sips >/dev/null; then
  [ "$(sips -g pixelWidth "$f" | awk '/pixelWidth/{print $2}')" = 1024 ]
  [ "$(sips -g hasAlpha "$f" | awk '/hasAlpha/{print $2}')" = no ]
else
  echo "skip: sips not available (macOS only)"
fi
