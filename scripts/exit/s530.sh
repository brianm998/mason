#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s530 exit test: a StepProgress message redraws only the progress views of the rows it names.
# Source checks always; the offscreen MonitorCore/MonitorUI tests on macOS (nothing is shown):
# a burst for the running tasks makes no sidebar publish and no row, pane or top-bar body run.
set -euo pipefail
cd "$(dirname "$0")/../.."
sw=clients/swift/Sources
f=$sw/MonitorCore/Model.swift
body=$(sed -n '/func apply(progresses/,/func appendLog/p' "$f")
! grep -n 'sidebar = \|taskDetail = ' <<<"$body" || { echo "s530: apply(progresses:) assigns the sidebar or task page" >&2; exit 1; }
test -f $sw/MonitorCore/ProgressStore.swift || { echo "s530: ProgressStore is missing" >&2; exit 1; }
if [ "$(uname)" = Darwin ] && command -v swift >/dev/null && [ -z "${MASON_SKIP_SWIFT:-}" ]; then
  out=$(mktemp); trap 'rm -f "$out"' EXIT
  if ! (cd clients/swift && swift test --filter 'RowRedrawTests|MonitorCoreTests') >"$out" 2>&1; then
    if grep -q 'Failed to clone repository\|Couldn.t fetch updates' "$out"; then
      echo "s530: swift packages unavailable here, offscreen tests skipped"
    else
      tail -40 "$out" >&2; exit 1
    fi
  fi
else
  echo "s530: not macOS, offscreen tests skipped"
fi
echo "s530: ok"
