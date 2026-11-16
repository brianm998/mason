#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Stage 451: the Android device sample has a manifest that names MainActivity as the launcher.
set -euo pipefail
cd "$(dirname "$0")/../.."
app=docs/examples/device-sample/android
m=$app/app/src/main/AndroidManifest.xml
[ -f "$m" ] || { echo "FAIL: $m is missing"; exit 1; }
grep -q 'SPDX-License-Identifier' "$m" || { echo "FAIL: manifest has no SPDX line"; exit 1; }
grep -q 'android:name="mason.sample.MainActivity"' "$m" || { echo "FAIL: MainActivity not declared"; exit 1; }
grep -q 'android.intent.action.MAIN' "$m" && grep -q 'android.intent.category.LAUNCHER' "$m" \
  || { echo "FAIL: MainActivity is not a launcher activity"; exit 1; }
echo "PASS: manifest declares mason.sample.MainActivity as the launcher"
if [ -n "${ANDROID_HOME:-}" ] && command -v gradle >/dev/null 2>&1; then
  gradle -p "$app" --console=plain :app:assembleDebug :app:testDebugUnitTest
  rm -rf "$app/.gradle" "$app/build" "$app/app/build"
  echo "PASS: assembleDebug and testDebugUnitTest"
else
  echo "SKIP: the build needs ANDROID_HOME and gradle on PATH; CI's device-sample-android job runs it"
fi
echo "s451: all checks passed"
