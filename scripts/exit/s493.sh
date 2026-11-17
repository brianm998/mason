#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s493 exit test: the Compose app tests (MascotTest and the other Skia-backed ones) pass.
set -euo pipefail
cd "$(dirname "$0")/../.."
if [ -n "${MASON_SKIP_COMPOSE:-}" ] || ! command -v gradle >/dev/null; then
    echo "== (Compose tests skipped: CI's monitor-compose job runs them)"
    exit 0
fi
(cd clients/compose && gradle cleanTest test --console=plain > "${TMPDIR:-/tmp}/s493-gradle.log" 2>&1) \
  || { grep -E "FAILED|tests completed" "${TMPDIR:-/tmp}/s493-gradle.log" >&2; exit 1; }
echo "s493 OK"
