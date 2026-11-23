#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1014 exit test: RemotePlacementTest passes with the body laid out in a Column.
set -euo pipefail
cd "$(dirname "$0")/../.."

if [ -n "${MASON_SKIP_COMPOSE:-}" ] || ! command -v gradle >/dev/null; then
    echo "== (Compose test skipped: MASON_SKIP_COMPOSE is set or gradle is missing)"
    exit 0
fi
(cd clients/compose && gradle --console=plain :app:test --tests 'mason.monitor.ui.RemotePlacementTest')
echo "s1014 passed"
