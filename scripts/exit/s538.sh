#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s538 exit test: with launchers and speech sinks that block until released, opening a session
# and loading the voices never block the UI executor (UiThreadTest, injected executors).
set -euo pipefail
cd "$(dirname "$0")/../.."

if [ -n "${MASON_SKIP_COMPOSE:-}" ] || ! command -v gradle >/dev/null; then
    echo "== (Compose tests skipped: CI's monitor-compose job runs them)"
    exit 0
fi
echo "== UiThreadTest"
(cd clients/compose && gradle :core:test --tests 'mason.monitor.UiThreadTest' --console=plain \
    > "${TMPDIR:-/tmp}/s538-gradle.log" 2>&1) \
  || { grep -E "FAILED|tests completed" "${TMPDIR:-/tmp}/s538-gradle.log" >&2; exit 1; }
echo "s538 OK"
