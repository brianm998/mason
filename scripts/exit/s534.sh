#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s534 exit test: the Swift sidebar list is lazy and the row meta line is one layout.
# Source checks always; the offscreen tests on macOS (nothing is shown).
set -euo pipefail
cd "$(dirname "$0")/../.."
f=clients/swift/Sources/MonitorUI/Sidebar.swift
grep -q 'LazyVStack(alignment: .leading, spacing: 0)' "$f" || { echo "s534: the sidebar list is not a LazyVStack" >&2; exit 1; }
! grep -q 'ViewThatFits' "$f" || { echo "s534: a ViewThatFits is back in the sidebar" >&2; exit 1; }
if [ "$(uname)" = Darwin ] && command -v swift >/dev/null && [ -z "${MASON_SKIP_SWIFT:-}" ]; then
  out=$(mktemp); trap 'rm -f "$out"' EXIT
  if ! (cd clients/swift && swift test --filter 'LazySidebarTests|RowRedrawTests|SidebarFitTests') >"$out" 2>&1; then
    if grep -q 'Failed to clone repository\|Couldn.t fetch updates' "$out"; then
      echo "s534: swift packages unavailable here, offscreen tests skipped"
    else
      tail -40 "$out" >&2; exit 1
    fi
  fi
else
  echo "s534: not macOS, offscreen tests skipped"
fi
echo "s534: ok"
