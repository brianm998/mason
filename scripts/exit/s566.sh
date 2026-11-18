#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Stage 566 exit test: the Swift test target compiles and the suite reports a summary.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
if [ -n "${MASON_SKIP_SWIFT:-}" ] || [ "$(uname)" != Darwin ] || ! command -v swift >/dev/null; then
  echo "s566 skipped: needs macOS and Swift"; exit 0
fi
cd clients/swift
swift build --build-tests 2>&1 | tail -1
out=$(swift test 2>&1 | grep -E "Executed .* tests" | head -1)
echo "$out"
case "$out" in *"(0 unexpected)"*) ;; *) echo "s566 FAIL: $out"; exit 1;; esac
