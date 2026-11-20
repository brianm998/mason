#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s661 exit test: the Swift monitor shows task references through LinkedText / TaskLink; the
# source scan passes with an allowlist of non-task ids only.
set -euo pipefail
cd "$(dirname "$0")/../.."
if [ "$(uname)" = Darwin ] && command -v swift >/dev/null && [ -z "${MASON_SKIP_SWIFT:-}" ]; then
  (cd clients/swift && swift test --filter "TaskReferenceLinkScanTests|LinkedTextTests")
fi
echo "s661 OK"
