#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s270 exit test: paused banners collapse to icons and Resume lifts exactly one pause.
#   1. the daemon test: a paused banner carries its pause's project and mode;
#   2. the Swift and Compose unit tests for collapse, prune and resume-exact-pause
#      (skipped when swift / gradle are missing, as in s96 and s79);
#   3. clients/features.toml has paused-collapse done on both clients.
set -euo pipefail
cd "$(dirname "$0")/../.."

cargo test -q -p mason-daemon --test pause_banners

if [ "$(uname)" = Darwin ] && command -v swift >/dev/null; then
  (cd clients/swift && swift test --filter PauseCollapseTests)
else
  echo "s270: swift not available; Swift tests skipped" >&2
fi
if [ -n "${MASON_SKIP_COMPOSE:-}" ] || ! command -v gradle >/dev/null; then
  echo "s270: gradle not available; Compose tests skipped" >&2
else
  (cd clients/compose && gradle :core:test --tests 'mason.monitor.PauseCollapseTest' --console=plain >"${TMPDIR:-/tmp}/s270-gradle.log" 2>&1) \
    || { tail -30 "${TMPDIR:-/tmp}/s270-gradle.log" >&2; exit 1; }
fi

python3 -I - <<'PY'
import tomllib
f = tomllib.load(open("clients/features.toml", "rb"))["feature"]["paused-collapse"]
assert f["swift"] == "done" and f["compose"] == "done", f
PY
echo "s270 OK"
