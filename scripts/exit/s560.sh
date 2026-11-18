#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s560 exit test: the Swift monitor's model is @Observable, no view observes the whole model, and an
# offscreen replay of announce claims, identical views and one changed header redraws only the bar.
set -euo pipefail
cd "$(dirname "$0")/../.."
if grep -rn 'ObservedObject var model\|ObservableObject' clients/swift/Sources/MonitorUI clients/swift/Sources/MonitorCore/Model.swift | grep -v SharedTick; then
  echo "s560: a view or the model still uses ObservableObject" >&2; exit 1
fi
if [ "$(uname)" = Darwin ] && command -v swift >/dev/null && [ -z "${MASON_SKIP_SWIFT:-}" ]; then
  out=$(mktemp); trap 'rm -f "$out"' EXIT
  if ! (cd clients/swift && swift test --filter 'FollowOnlyWhatTheyShowTests|RowRedrawTests') >"$out" 2>&1; then
    if grep -q 'Failed to clone repository\|Couldn.t fetch updates' "$out"; then
      echo "s560: swift packages unavailable here, offscreen tests skipped"
    else
      tail -40 "$out" >&2; exit 1
    fi
  fi
else
  echo "s560: not macOS, offscreen tests skipped"
fi
echo "s560: ok"
