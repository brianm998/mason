#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s494 exit test: MascotTest passes, so the test JVM loads Skia's native library.
set -euo pipefail
cd "$(dirname "$0")/../.."
if [ -n "${MASON_SKIP_COMPOSE:-}" ] || ! command -v gradle >/dev/null; then
    echo "s494 skipped: MASON_SKIP_COMPOSE is set or gradle is missing; CI's monitor-compose job runs the tests"
    exit 0
fi
(cd clients/compose && gradle :app:test --tests 'mason.monitor.ui.MascotTest' --console=plain > "${TMPDIR:-/tmp}/s494-gradle.log" 2>&1) \
  || { grep -E "FAILED|tests completed|UnsatisfiedLinkError" "${TMPDIR:-/tmp}/s494-gradle.log" >&2; exit 1; }
echo "s494 OK"
