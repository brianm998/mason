#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s673 exit test: mason://section/proposed and mason://section/needs_attention resolve to
# pages, and a double click on either tile with several tasks opens that page. Checks the
# page names in both clients, then runs the Swift monitor core, phone and offscreen render tests.
set -euo pipefail
cd "$(dirname "$0")/../.."
for f in clients/swift/Sources/MonitorCore/Navigation.swift clients/compose/core/src/main/kotlin/mason/monitor/Navigation.kt; do
  for n in needs_attention proposed; do
    grep -q "\"$n\"" "$f" || { echo "s673: $n missing from $f" >&2; exit 1; }
  done
done
if [ "$(uname)" = Darwin ] && command -v swift >/dev/null && [ -z "${MASON_SKIP_SWIFT:-}" ]; then
  out=$(mktemp); trap 'rm -f "$out"' EXIT
  if ! (cd clients/swift && swift test --filter 'testDoubleClickOnStuckOrProposed|testSectionUrlsRoundTrip|testATileTapPushes|ProposedPageTests') >"$out" 2>&1; then
    if grep -q 'Failed to clone repository\|Couldn.t fetch updates' "$out"; then
      echo "s673: swift packages unavailable here, tests skipped"
    else
      tail -40 "$out" >&2; exit 1
    fi
  fi
else
  echo "s673: not macOS, tests skipped"
fi
echo "s673: ok"
