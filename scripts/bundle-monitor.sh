#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Build the macOS monitor as an application bundle, so the `mason://` links
# (from notifications and `open mason://task/12`) have an app to open.
# Usage: bundle-monitor.sh [--universal] [OUTPUT_DIR]   (default: build/, host architecture)
#   --universal  build arm64 and x86_64 into one binary (releases; the default stays host-only,
#                which is what the in-app update runs)
# Environment: SIGN_APP=1 signs the app (hardened runtime, secure timestamp) with SIGN_IDENTITY
#   (default "Developer ID Application"; "-" is an ad-hoc signature, for testing the path).
#   ENTITLEMENTS=<file> adds an entitlements file; none is needed today (see docs/RELEASING.md).
set -euo pipefail
cd "$(dirname "$0")/.."
arch_args=()
out=""
for a in "$@"; do
  case "$a" in
    --universal) arch_args=(--arch arm64 --arch x86_64) ;;
    -*) echo "bundle-monitor: unknown option $a" >&2; exit 2 ;;
    *) out="$a" ;;
  esac
done
out="${out:-build}"
# The commit this monitor is, for noticing when it is out of date: given (a build by
# the daemon, from a clean copy), or the last commit that changed it here. A checkout
# with uncommitted changes to it is a development build, which never asks to update.
paths=(clients/swift scripts/bundle-monitor.sh)
build_sha="${MASON_BUILD_SHA:-}"
if [ -z "$build_sha" ] && git rev-parse --git-dir >/dev/null 2>&1 && git diff --quiet HEAD -- "${paths[@]}"; then
  build_sha=$(git rev-list -1 HEAD -- "${paths[@]}")
fi
# One version for every build: MASON_VERSION, else the workspace version in Cargo.toml.
version="${MASON_VERSION:-$(awk '/^\[workspace\.package\]/{f=1;next} /^\[/{f=0} f && /^version *=/{gsub(/[" ]/,"",$0); sub(/^version=/,"",$0); print; exit}' Cargo.toml)}"
[ -n "$version" ] || { echo "bundle-monitor: no version (set MASON_VERSION)" >&2; exit 1; }
(cd clients/swift && swift build -c release --product MasonMonitor ${arch_args[@]+"${arch_args[@]}"})
bin=$(cd clients/swift && swift build -c release ${arch_args[@]+"${arch_args[@]}"} --show-bin-path)
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
# (A host build lays the bundle out flat; a universal one, built by Xcode's engine, has Contents/Resources.)
mascot="$app/Contents/Resources/MasonMonitor_MasonMascot.bundle"
[ -d "$mascot/Art" ] || [ -d "$mascot/Contents/Resources/Art" ] || {
  echo "bundle-monitor: the mascot resource bundle is missing from $app" >&2; exit 1; }
# The app icon: an .icns made from the 1024 px macOS-shaped icon (rounded, on Apple's grid;
# macOS does not mask icons itself). Regenerate it with scripts/gen-app-icon-macos.swift.
icon_src="clients/swift/Sources/MasonMascot/Art/brand/app_icon_macos.png"
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
  <key>CFBundleShortVersionString</key><string>@VERSION@</string>
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
sed -i.bak "s/@VERSION@/$version/" "$app/Contents/Info.plist" && rm "$app/Contents/Info.plist.bak"
[ -z "$build_sha" ] || plutil -insert MasonBuild -string "$build_sha" "$app/Contents/Info.plist"
plutil -lint "$app/Contents/Info.plist" >/dev/null
if [ -n "${SIGN_APP:-}" ]; then
  identity="${SIGN_IDENTITY:-Developer ID Application}"
  sign_args=(--force --options runtime --timestamp --sign "$identity")
  [ "$identity" != "-" ] || sign_args=(--force --options runtime --sign -)
  # Inside out: nested bundles first, then the app (no --deep, which Apple deprecates).
  for res in "$app"/Contents/Resources/*.bundle; do
    [ -d "$res" ] || continue
    codesign "${sign_args[@]}" "$res"
  done
  if [ -n "${ENTITLEMENTS:-}" ]; then sign_args+=(--entitlements "$ENTITLEMENTS"); fi
  codesign "${sign_args[@]}" "$app"
  codesign --verify --strict --verbose=2 "$app"
fi
echo "built $app (register it with: open $app; then mason://task/12 opens in it)"
