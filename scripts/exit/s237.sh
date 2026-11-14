#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s237 exit test: the Mac monitor's sidebar rows are not redrawn on every model change.
# Runs the MonitorUI offscreen tests (nothing is ordered in or shown): a recorded stream of
# 2,000 messages for 25 running tasks must cost no more than twice the number of row-changing
# messages in RowView body evaluations. Swift builds only on macOS, so elsewhere the source
# checks run alone.
set -euo pipefail
cd "$(dirname "$0")/../.."
sw=clients/swift/Sources
! grep -n '@ObservedObject var model' <(sed -n '/^struct RowView/,/^}/p' $sw/MonitorUI/Sidebar.swift) \
  || { echo "s237: RowView observes the whole model" >&2; exit 1; }
! grep -n 'TimelineView' $sw/MonitorUI/Components.swift || { echo "s237: a timer per row is back" >&2; exit 1; }
! grep -n 'id: \\.offset' <(grep -n 'sidebarEntries' $sw/MonitorUI/Sidebar.swift) || { echo "s237: sidebar keyed by position" >&2; exit 1; }
if [ "$(uname)" = Darwin ] && command -v swift >/dev/null; then
  out=$(mktemp); trap 'rm -f "$out"' EXIT
  if ! (cd clients/swift && swift test --filter 'RowRedrawTests|MonitorCoreTests') >"$out" 2>&1; then
    # A sandbox with no network cannot fetch swift-protobuf: that is not a red test.
    if grep -q 'Failed to clone repository\|Couldn.t fetch updates' "$out"; then
      echo "s237: swift packages unavailable here, offscreen tests skipped"
    else
      tail -40 "$out" >&2; exit 1
    fi
  else
    grep 's237:' "$out" || true
  fi
else
  echo "s237: not macOS, offscreen tests skipped"
fi
echo "s237: ok"
