#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s492 exit test: MascotTest passes (Skia's native library loads in the test JVM).
set -euo pipefail
cd "$(dirname "$0")/../.."

if [ -n "${MASON_SKIP_COMPOSE:-}" ] || ! command -v gradle >/dev/null; then
    echo "== (Compose tests skipped: CI's monitor-compose job runs them)"
    exit 0
fi
(cd clients/compose && gradle --console=plain :app:test --tests 'mason.monitor.ui.MascotTest' \
    > "${TMPDIR:-/tmp}/s492-gradle.log" 2>&1) \
  || { grep -E "FAILED|tests completed|UnsatisfiedLinkError" "${TMPDIR:-/tmp}/s492-gradle.log" >&2; exit 1; }
echo "s492 OK"
