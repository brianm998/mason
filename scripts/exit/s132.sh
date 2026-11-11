#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s132 exit test: the macOS icon variant exists, is shaped, and the bundle script uses it.
set -euo pipefail
cd "$(dirname "$0")/../.."
f=clients/swift/Sources/MasonMascot/Art/brand/app_icon_macos.png
grep -q 'app_icon_macos.png' scripts/bundle-monitor.sh
test -f scripts/gen-app-icon-macos.swift
test -f "$f"
if command -v sips >/dev/null; then
  [ "$(sips -g hasAlpha "$f" | awk '/hasAlpha/{print $2}')" = yes ]
  [ "$(sips -g pixelWidth "$f" | awk '/pixelWidth/{print $2}')" = 1024 ]
  [ "$(sips -g pixelHeight "$f" | awk '/pixelHeight/{print $2}')" = 1024 ]
fi
# iOS stays opaque
if command -v sips >/dev/null; then
  [ "$(sips -g hasAlpha clients/swift/ios/MasonIOS/AppIcon-1024.png | awk '/hasAlpha/{print $2}')" = no ]
fi
echo "s132 OK"
