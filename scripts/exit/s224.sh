#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Stage 224 exit test: the universal MASON .dmg builds (unsigned) and holds a universal app of
# the workspace version. Never launches the app. Needs macOS and Swift; skipped elsewhere and
# when MASON_SKIP_SWIFT is set.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
if [ -n "${MASON_SKIP_SWIFT:-}" ] || [ "$(uname)" != Darwin ] || ! command -v swift >/dev/null; then
  echo "s224 skipped: needs macOS and Swift"; exit 0
fi
out=$(mktemp -d /tmp/mason-s224.XXXXXX); mnt="$out/mnt"; mkdir "$mnt"
cleanup() { hdiutil detach -quiet "$mnt" 2>/dev/null || true; rm -rf "$out"; }
trap cleanup EXIT
bash scripts/package/macos-dmg.sh "$out" | tail -1
version=$(bash -c 'awk "/^\[workspace\.package\]/{f=1;next} /^\[/{f=0} f && /^version *=/{gsub(/[\" ]/,\"\"); sub(/^version=/,\"\"); print; exit}" Cargo.toml')
dmg="$out/MASON-$version-macos.dmg"
[ -f "$dmg" ] || { echo "no $dmg" >&2; exit 1; }
hdiutil attach -nobrowse -readonly -mountpoint "$mnt" "$dmg" >/dev/null
archs=$(/usr/bin/lipo -archs "$mnt/MASON.app/Contents/MacOS/MasonMonitor")
case "$archs" in *arm64*) ;; *) echo "missing arm64: $archs" >&2; exit 1 ;; esac
case "$archs" in *x86_64*) ;; *) echo "missing x86_64: $archs" >&2; exit 1 ;; esac
[ "$(plutil -extract CFBundleShortVersionString raw "$mnt/MASON.app/Contents/Info.plist")" = "$version" ]
[ -L "$mnt/Applications" ]
echo "s224 ok: $dmg ($archs)"
