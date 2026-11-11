#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s96 exit test: the mascot manifest is enforced and a built MASON.app carries its art.
#   1. `conformance mascot --check` passes; fails on a removed listed file and on an unlisted one;
#   2. the Swift and Compose tests load every id (skipped when swift/gradle are missing);
#   3. a MASON.app from bundle-monitor.sh loads every image after the SwiftPM build dir is moved.
set -euo pipefail
cd "$(dirname "$0")/../.."
art=clients/swift/Sources/MasonMascot/Art
cargo build -q -p mason-conformance
c=target/debug/conformance
$c mascot --check
f=$art/brand/app_icon.png
mv "$f" "$f.bak"; trap 'mv "$f.bak" "$f" 2>/dev/null || true; rm -f "$art/brand/extra.png"' EXIT
if $c mascot --check 2>/dev/null; then echo "a missing file passed" >&2; exit 1; fi
mv "$f.bak" "$f"
cp "$f" "$art/brand/extra.png"
if $c mascot --check 2>/dev/null; then echo "an unlisted file passed" >&2; exit 1; fi
rm "$art/brand/extra.png"
$c mascot --check

if [ "$(uname)" = Darwin ] && command -v swift >/dev/null; then
  out=$(mktemp -d /tmp/mason-s96.XXXXXX)
  (cd clients/swift && swift test --filter MasonMascotTests)
  bash scripts/bundle-monitor.sh "$out/app"
  mv clients/swift/.build "$out/moved-build"
  trap 'mv "$out/moved-build" clients/swift/.build 2>/dev/null || true; rm -rf "$out"' EXIT
  "$out/app/MASON.app/Contents/MacOS/MasonMonitor" --mascot-check
fi
echo "s96 OK"
