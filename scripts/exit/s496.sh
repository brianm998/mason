#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s496 exit test: the Skia-decoding Compose tests (TopBarTest, MascotTest) load the native library.
set -euo pipefail
cd "$(dirname "$0")/../.."

if [ -n "${MASON_SKIP_COMPOSE:-}" ] || ! command -v gradle >/dev/null; then
    echo "== (Compose tests skipped: CI's monitor-compose job runs them)"
    exit 0
fi
echo "== TopBarTest and MascotTest"
(cd clients/compose && gradle :app:test --tests 'mason.monitor.ui.TopBarTest' \
    --tests 'mason.monitor.ui.MascotTest' --console=plain > "${TMPDIR:-/tmp}/s496-gradle.log" 2>&1) \
  || { grep -E "FAILED|tests completed" "${TMPDIR:-/tmp}/s496-gradle.log" >&2; exit 1; }
echo "s496 OK"
