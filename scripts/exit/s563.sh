#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s563 exit test: UiThreadTest (including isExpandedReadsEachKeyOnce) passes.
set -euo pipefail
cd "$(dirname "$0")/../.."

if [ -n "${MASON_SKIP_COMPOSE:-}" ] || ! command -v gradle >/dev/null; then
    echo "== (Compose tests skipped: MASON_SKIP_COMPOSE is set or gradle is missing)"
    exit 0
fi
echo "== UiThreadTest"
(cd clients/compose && gradle :core:test --tests 'mason.monitor.UiThreadTest' --console=plain > "${TMPDIR:-/tmp}/s563-gradle.log" 2>&1) \
  || { grep -E "FAILED|tests completed" "${TMPDIR:-/tmp}/s563-gradle.log" >&2; exit 1; }
echo "s563 OK"
