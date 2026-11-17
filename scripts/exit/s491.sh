#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s491 exit test: the Compose editor snapshot test passes (Skiko loads in the test JVM).
set -euo pipefail
cd "$(dirname "$0")/../.."

if [ -n "${MASON_SKIP_COMPOSE:-}" ] || ! command -v gradle >/dev/null; then
    echo "== (Compose tests skipped: MASON_SKIP_COMPOSE is set or gradle is missing)"
    exit 0
fi
echo "== Compose editor test"
(cd clients/compose && gradle :app:test --tests 'mason.monitor.ui.EditorSnapshotTest' --console=plain > "${TMPDIR:-/tmp}/s491-gradle.log" 2>&1) \
  || { grep -E "FAILED|tests completed|UnsatisfiedLinkError" "${TMPDIR:-/tmp}/s491-gradle.log" >&2; exit 1; }
echo "s491 OK"
