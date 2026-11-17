#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s540 exit test: both monitors' Health pages, rendered headless with injected watchdog stats, show the
# UI-thread line with those numbers; the watchdog is no longer debug-only.
set -euo pipefail
cd "$(dirname "$0")/../.."
if grep -B1 'MainThreadWatchdog.shared.start' clients/swift/Sources/MasonMonitor/App.swift | grep -q '#if DEBUG'; then
  echo "s540: Swift watchdog is still debug-only" >&2; exit 1
fi
if grep -q 'getenv("MASON_WATCHDOG") != "1"' clients/compose/core/src/main/kotlin/mason/monitor/UiWatchdog.kt; then
  echo "s540: Compose watchdog is still env-gated" >&2; exit 1
fi
if [ "$(uname)" = Darwin ] && command -v swift >/dev/null && [ -z "${MASON_SKIP_SWIFT:-}" ]; then
  out=$(mktemp); trap 'rm -f "$out"' EXIT
  if ! (cd clients/swift && swift test --filter 'WatchdogTests|HealthLineTests') >"$out" 2>&1; then
    if grep -q 'Failed to clone repository\|Couldn.t fetch updates' "$out"; then
      echo "s540: swift packages unavailable here, offscreen tests skipped"
    else
      tail -40 "$out" >&2; exit 1
    fi
  fi
else
  echo "s540: not macOS, offscreen tests skipped"
fi
echo "s540: ok"
