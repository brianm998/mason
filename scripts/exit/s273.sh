#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s273 exit test: SidebarFitTests compiles against RowView's current initializer.
set -euo pipefail
cd "$(dirname "$0")/../.."
! grep -n 'RowView(model:' clients/swift/Tests/MonitorUITests/SidebarFitTests.swift \
  || { echo "s273: stale RowView(model:) call" >&2; exit 1; }
if [ "$(uname)" = Darwin ] && command -v swift >/dev/null; then
  out=$(mktemp); trap 'rm -f "$out"' EXIT
  if ! (cd clients/swift && swift test --filter SidebarFitTests) >"$out" 2>&1; then
    if grep -q 'Failed to clone repository\|Couldn.t fetch updates' "$out"; then
      echo "s273: swift packages unavailable here, tests skipped"
    else
      tail -40 "$out" >&2; exit 1
    fi
  fi
else
  echo "s273: not macOS, tests skipped"
fi
echo "s273: ok"
