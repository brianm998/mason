#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1073 exit test: the Swift settle waits on view seq (decision 1062).
set -euo pipefail
cd "$(dirname "$0")/../.."

grep -q '^Summary: ' docs/stages/s1073.md
grep -q 'lastViewSeq' clients/swift/Sources/MonitorCore/Model.swift
grep -q 'model.lastViewSeq >= target' clients/swift/Sources/MonitorTool/Tool.swift
if [ "$(uname)" = Darwin ] && command -v swift >/dev/null && [ -z "${MASON_SKIP_SWIFT:-}" ]; then
    (cd clients/swift && swift test --filter testLastViewSeqMovesOnViewsNotOnEvents)
else
    echo "skipping the Swift test: needs macOS with swift"
fi
echo "OK s1073"
