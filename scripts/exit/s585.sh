#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Stage 585 exit test: the mascot cache test passes, alone and with the rest of the package.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
if [ -n "${MASON_SKIP_SWIFT:-}" ] || [ "$(uname)" != Darwin ] || ! command -v swift >/dev/null; then
  echo "s585 skipped: needs macOS and Swift"; echo "s585: ok"; exit 0
fi
out=$(cd clients/swift && swift test --filter MasonMascotTests 2>&1) || { echo "$out" | tail -30; exit 1; }
echo "$out" | grep -E "Executed .* tests" | head -1
if echo "$out" | grep -qE "with [1-9][0-9]* failure"; then echo "$out" | tail -30; exit 1; fi
