#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Build the macOS monitor as an application bundle, so the `mason://` links
# (from notifications and `open mason://task/12`) have an app to open.
# Usage: bundle-monitor.sh [OUTPUT_DIR]   (default: build/)
set -euo pipefail
cd "$(dirname "$0")/.."
out="${1:-build}"
# The commit this monitor is, for noticing when it is out of date: given (a build by
# the daemon, from a clean copy), or the last commit that changed it here. A checkout
# with uncommitted changes to it is a development build, which never asks to update.
paths=(clients/swift scripts/bundle-monitor.sh)
build_sha="${MASON_BUILD_SHA:-}"
if [ -z "$build_sha" ] && git rev-parse --git-dir >/dev/null 2>&1 && git diff --quiet HEAD -- "${paths[@]}"; then
  build_sha=$(git rev-list -1 HEAD -- "${paths[@]}")
fi
(cd clients/swift && swift build -c release --product MasonMonitor)
bin=$(cd clients/swift && swift build -c release --show-bin-path)
app="$out/MASON.app"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS"
cp "$bin/MasonMonitor" "$app/Contents/MacOS/MasonMonitor"
cat > "$app/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleIdentifier</key><string>org.mason.monitor</string>
  <key>CFBundleName</key><string>MASON</string>
  <key>CFBundleExecutable</key><string>MasonMonitor</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>0.0.0</string>
  <key>LSMinimumSystemVersion</key><string>14.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>CFBundleURLTypes</key>
  <array>
    <dict>
      <key>CFBundleURLName</key><string>MASON links</string>
      <key>CFBundleURLSchemes</key><array><string>mason</string></array>
    </dict>
  </array>
</dict>
</plist>
PLIST
[ -z "$build_sha" ] || plutil -insert MasonBuild -string "$build_sha" "$app/Contents/Info.plist"
plutil -lint "$app/Contents/Info.plist" >/dev/null
echo "built $app (register it with: open $app; then mason://task/12 opens in it)"
