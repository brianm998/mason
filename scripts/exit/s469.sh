#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s469 exit test: the Android phone app builds.
#  1. The parity gate (`conformance features clients/features.toml`) passes with `android` as the
#     fourth client, and every feature has an `android` entry.
#  2. The manifest asks for internet, local-network discovery and the camera, is portrait, and
#     takes `mason://` links.
#  3. The debug APK builds and the Android unit tests (Keystore-backed token store, pairing screen,
#     connection order, navigation) pass, with at least one test run.
# Step 3 needs the Android SDK, Java and Gradle. Where they are missing it is SKIPPED and says so in
# its last line: a skip is never reported as a pass. Real-phone pairing over Wi-Fi is the owner's
# check (s469-real.sh, docs/stages/s469.md), not run here.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

echo "== 1. the parity gate, with android"
cargo build -q -p mason-conformance --bins
out=$(target/debug/conformance features clients/features.toml 2>&1) || { echo "$out" >&2; echo "FAIL: the feature check failed" >&2; exit 1; }
echo "$out"
echo "$out" | grep -q "in android " || { echo "FAIL: no android column in the report" >&2; exit 1; }
features=$(grep -c '^\[feature\.' clients/features.toml)
entries=$(grep -c '^android = ' clients/features.toml)
[ "$features" = "$entries" ] || { echo "FAIL: $features features but $entries android entries" >&2; exit 1; }

echo "== 2. the manifest"
m=clients/compose/android/src/main/AndroidManifest.xml
for want in 'android.permission.INTERNET' 'android.permission.CHANGE_WIFI_MULTICAST_STATE' \
  'android.permission.NEARBY_WIFI_DEVICES' 'android.permission.CAMERA' 'screenOrientation="portrait"' 'android:scheme="mason"'; do
  grep -q "$want" "$m" || { echo "FAIL: $m has no $want" >&2; exit 1; }
done

echo "== 3. the debug APK and the Android unit tests"
sdk=""
for d in "${ANDROID_HOME:-}" "${ANDROID_SDK_ROOT:-}" "$(sed -n 's/^sdk.dir=//p' clients/compose/local.properties 2>/dev/null | head -1)"; do
  if [ -n "$d" ] && [ -d "$d" ]; then sdk=$d; break; fi
done
why=""
if [ -n "${MASON_SKIP_COMPOSE:-}" ]; then why="MASON_SKIP_COMPOSE is set"
elif ! command -v java >/dev/null || ! command -v gradle >/dev/null; then why="Java and Gradle are needed"
elif [ -z "$sdk" ]; then why="no Android SDK (set ANDROID_HOME, or sdk.dir in clients/compose/local.properties)"
fi
if [ -n "$why" ]; then
  echo "s469 SKIPPED the Android build and unit tests: $why. The parity gate and manifest checks passed."
  exit 0
fi
export ANDROID_HOME="$sdk"
log=$(mktemp /tmp/mason-s469.XXXXXX)
trap 'rm -f "$log"' EXIT
(cd clients/compose && gradle --no-daemon -q -Pmason.android=true :android:assembleDebug :android:testDebugUnitTest) >"$log" 2>&1 \
  || { tail -40 "$log" >&2; echo "FAIL: the Android build or its unit tests failed" >&2; exit 1; }
apk=$(ls clients/compose/android/build/outputs/apk/debug/*.apk 2>/dev/null | head -1)
[ -n "$apk" ] && [ -s "$apk" ] || { echo "FAIL: no debug APK was built" >&2; exit 1; }
python3 -I - <<'PY'
import glob, re
t = f = 0
for p in glob.glob("clients/compose/android/build/test-results/testDebugUnitTest/*.xml"):
    m = re.search(r'tests="(\d+)" skipped="\d+" failures="(\d+)" errors="(\d+)"', open(p).read())
    t += int(m.group(1)); f += int(m.group(2)) + int(m.group(3))
print(f"{t} Android unit tests, {f} failures")
assert t > 0 and f == 0
PY
echo "s469 OK: $apk"
