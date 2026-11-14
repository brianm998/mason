#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Build MASON-<ver>-macos.dmg: the universal Swift monitor app and an /Applications link.
# Usage: macos-dmg.sh [OUTPUT_DIR]   (default: build/package)
# With SIGN_APP set the app and the dmg are signed (SIGN_IDENTITY, see bundle-monitor.sh) and
# the dmg is notarized and stapled with scripts/package/notarize.sh (set NOTARIZE=0 to skip).
# Never launches the app: the bundle is checked with codesign, plutil and lipo.
set -euo pipefail
cd "$(dirname "$0")/../.."
out="$(cd "$(dirname "${1:-build/package}/x")" 2>/dev/null && pwd || true)"
[ -n "$out" ] || { mkdir -p "${1:-build/package}"; out="$(cd "${1:-build/package}" && pwd)"; }
stage="$out/dmg-stage"
rm -rf "$stage"; mkdir -p "$stage"
bash scripts/bundle-monitor.sh --universal "$stage"
app="$stage/MASON.app"
archs=$(/usr/bin/lipo -archs "$app/Contents/MacOS/MasonMonitor")
case "$archs" in *arm64*x86_64*|*x86_64*arm64*) ;; *) echo "macos-dmg: not universal ($archs)" >&2; exit 1 ;; esac
plutil -lint "$app/Contents/Info.plist" >/dev/null
version=$(plutil -extract CFBundleShortVersionString raw "$app/Contents/Info.plist")
if [ -n "${SIGN_APP:-}" ]; then codesign --verify --strict "$app"; fi
ln -s /Applications "$stage/Applications"
dmg="$out/MASON-$version-macos.dmg"
rm -f "$dmg"
hdiutil create -quiet -volname "MASON $version" -srcfolder "$stage" -ov -format UDZO "$dmg"
rm -rf "$stage"
if [ -n "${SIGN_APP:-}" ]; then
  identity="${SIGN_IDENTITY:-Developer ID Application}"
  codesign --force --timestamp --sign "$identity" "$dmg"
  codesign --verify --verbose=2 "$dmg"
  if [ "${NOTARIZE:-1}" != 0 ] && [ "$identity" != "-" ]; then bash scripts/package/notarize.sh "$dmg"; fi
fi
echo "built $dmg ($archs, version $version)"
