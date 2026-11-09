#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s79 exit test: the sidebar width logic and the pinned/scrolling split are tested in both
# clients, the generated tokens are current and the feature table is in parity.
set -euo pipefail
cd "$(dirname "$0")/../.."

echo "== tokens are current"
cargo run -q -p mason-conformance -- tokens --check
echo "== feature parity"
cargo run -q -p mason-conformance -- features
echo "== Compose core and app tests"
(cd clients/compose && gradle test -q)
if command -v swift >/dev/null; then
    echo "== Swift tests"
    (cd clients/swift && swift test --filter MonitorCoreTests)
else
    echo "== (swift not installed: the Swift suite is CI's monitor-swift job)"
fi
echo "s79 OK (the hand checks are in s79-manual.sh)"
