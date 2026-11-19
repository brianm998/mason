#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s631 exit test: the offscreen body-count suites host their views in a never-ordered window.
set -euo pipefail
cd "$(dirname "$0")/../.."
d=clients/swift/Tests/MonitorUITests
for t in RowRedrawTests FollowOnlyWhatTheyShowTests LazySidebarTests; do
  f=$d/$t.swift
  grep -q 'NSWindow(' "$f" || { echo "s631: $t has no window host" >&2; exit 1; }
  grep -q 'isReleasedWhenClosed = false' "$f" || { echo "s631: $t window is released on close" >&2; exit 1; }
  ! grep -q 'orderFront\|makeKey' "$f" || { echo "s631: $t orders a window in" >&2; exit 1; }
done
if [ "$(uname)" = Darwin ] && command -v swift >/dev/null && [ -z "${MASON_SKIP_SWIFT:-}" ]; then
  out=$(mktemp); trap 'rm -f "$out"' EXIT
  if ! (cd clients/swift && swift test --filter 'RowRedrawTests|FollowOnlyWhatTheyShowTests|LazySidebarTests') >"$out" 2>&1; then
    if grep -q 'Failed to clone repository\|Couldn.t fetch updates' "$out"; then
      echo "s631: swift packages unavailable here, offscreen tests skipped"
    else
      tail -40 "$out" >&2; exit 1
    fi
  fi
else
  echo "s631: not macOS, offscreen tests skipped"
fi
echo "s631: ok"
