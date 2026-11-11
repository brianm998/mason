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
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$bin/MasonMonitor" "$app/Contents/MacOS/MasonMonitor"
# SwiftPM puts each target's resources in a bundle beside the binary. Without them in the
# app the mascot art is gone as soon as the build directory is (the in-app update runs this
# script too). `Mascot` looks in Contents/Resources first.
for res in "$bin"/*.bundle; do
  [ -d "$res" ] || continue
  cp -R "$res" "$app/Contents/Resources/"
done
[ -d "$app/Contents/Resources/MasonMonitor_MasonMascot.bundle/Art" ] || {
  echo "bundle-monitor: the mascot resource bundle is missing from $app" >&2; exit 1; }
# The app icon: an .icns made from the 1024 px mascot icon (Finder and the Dock read it).
icon_src="clients/swift/Sources/MasonMascot/Art/brand/app_icon.png"
iconset=$(mktemp -d)/MASON.iconset
mkdir -p "$iconset"
for s in 16 32 128 256 512; do
  sips -z $s $s "$icon_src" --out "$iconset/icon_${s}x${s}.png" >/dev/null
  sips -z $((s * 2)) $((s * 2)) "$icon_src" --out "$iconset/icon_${s}x${s}@2x.png" >/dev/null
done
iconutil -c icns "$iconset" -o "$app/Contents/Resources/MASON.icns"
rm -rf "$(dirname "$iconset")"
cat > "$app/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleIdentifier</key><string>org.mason.monitor</string>
  <key>CFBundleName</key><string>MASON</string>
  <key>CFBundleIconFile</key><string>MASON</string>
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
