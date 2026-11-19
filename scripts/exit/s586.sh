#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s586 exit test: the notification bar's MonitorCore and MonitorUI tests run by name, and none is
# missing or skipped. Swift runs on macOS only (skipped elsewhere or with MASON_SKIP_SWIFT).
set -euo pipefail
cd "$(dirname "$0")/../.."
if [ "$(uname)" != Darwin ] || ! command -v swift >/dev/null || [ -n "${MASON_SKIP_SWIFT:-}" ]; then
  echo "s586 skipped: Swift is not available here"
  exit 0
fi
out=$(mktemp /tmp/s586.XXXXXX)
trap 'rm -f "$out"' EXIT
cd clients/swift
for t in \
  NotificationRowsTests/testKeysSurviveReorderingAndChangeWithTheText \
  NotificationRowsTests/testDismissHidesOnlyThatRow \
  NotificationRowsTests/testPruneForgetsAGoneRowAndItShowsAgainWhenItReturns \
  NotificationRowsTests/testShowHiddenClearsOnlyThePresentOnes \
  NotificationRowsTests/testDismissalsAndHeightSurviveANewModel \
  NotificationRowsTests/testClientRowsKeepTheirIDs \
  NotificationRowsTests/testHeightClamp \
  NotificationBarTests/testTenRowsAreAsTallAsThreePlusTheHandle \
  NotificationBarTests/testThreeRowsHaveNoScrollViewAndNoHandle \
  NotificationBarTests/testEveryRowHasADismissButton \
  NotificationBarTests/testShowHiddenButtonAppearsAndTheTopRowEndsBeforeIt \
  NotificationBarTests/testPauseRowHasNoXmarkButItsDismiss; do
  swift test --filter "$t" >"$out" 2>&1 || { cat "$out"; echo "s586: $t failed" >&2; exit 1; }
  grep -Eq "Test Case .*${t#*/}.* passed" "$out" || { cat "$out"; echo "s586: $t did not run or was skipped" >&2; exit 1; }
  grep -q "skipped" "$out" && { echo "s586: $t was skipped" >&2; exit 1; }
done
echo "s586 OK"
