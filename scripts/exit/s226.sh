#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s226 exit test: the picker chooses the right files per platform, and index.html renders the
# fixture release (read from the DOM in headless Chrome, when installed; nothing on screen).
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
bash scripts/tests/pick.sh
for f in site/index.html site/pick.js site/app.js site/style.css site/assets/banner.png site/assets/icon.png site/assets/NOTICE.md; do
  [ -s "$f" ] || { echo "missing $f" >&2; exit 1; }
done
chrome="/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
[ -x "$chrome" ] || chrome=$(command -v google-chrome chromium 2>/dev/null | head -n1 || true)
if [ -z "$chrome" ]; then echo "no headless Chrome: DOM check not run"; exit 0; fi
ua=$(python3 -c 'import urllib.parse;print(urllib.parse.quote("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 Chrome/126.0 Safari/537.36"))')
prof=$(mktemp -d); trap 'rm -rf "$prof"' EXIT
dom=$(timeout 90 "$chrome" --headless=new --disable-gpu --user-data-dir="$prof" --allow-file-access-from-files \
  --virtual-time-budget=5000 --dump-dom "file://$PWD/site/index.html?release=test/fixture.json&ua=$ua" 2>/dev/null)
echo "$dom" | grep -q 'MASON-Monitor-1.2.3-windows.msi' || { echo "windows monitor link missing from DOM" >&2; exit 1; }
echo "$dom" | grep -q 'mason-1.2.3-x86_64-windows-setup.msi' || { echo "windows cli link missing from DOM" >&2; exit 1; }
echo "$dom" | grep -q 'macos.dmg' && { echo "wrong platform file in DOM" >&2; exit 1; }
echo "s226 ok"
