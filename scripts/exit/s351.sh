#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s351 exit test: the Kotlin monitor's packaging icons exist and match the brand art.
set -euo pipefail
cd "$(dirname "$0")/../../clients/compose"
brand=../swift/Sources/MasonMascot/Art/brand
for f in packaging/icons/mason-monitor.png packaging/icons/mason-monitor.ico \
  packaging/icons/mason-monitor.icns android/src/main/AndroidManifest.xml \
  android/src/main/res/mipmap-anydpi/ic_launcher.xml \
  android/src/main/res/drawable/ic_launcher_foreground.xml \
  android/src/main/res/drawable-nodpi/ic_launcher_art.png; do
  test -s "$f" || { echo "missing $f" >&2; exit 1; }
done
cmp "$brand/app_icon.png" packaging/icons/mason-monitor.png
python3 -I packaging/make-icns.py --check
python3 -I packaging/make-android-icons.py --check
# The .ico wraps one 256x256 PNG.
python3 -I - <<'PY'
d = open("packaging/icons/mason-monitor.ico", "rb").read()
assert d[:6] == b"\0\0\1\0\1\0" and d[22:30] == b"\x89PNG\r\n\x1a\n", "bad .ico"
assert d[6] == 0 and d[7] == 0, "ico is not 256x256"
PY
grep -q 'macOS {' desktop/build.gradle.kts
grep -q 'mason-monitor.icns' desktop/build.gradle.kts
grep -q 'ic_launcher' android/src/main/AndroidManifest.xml
if grep -q 'include(":android")' settings.gradle.kts; then echo "android must stay out of settings" >&2; exit 1; fi
echo "s351 OK"
