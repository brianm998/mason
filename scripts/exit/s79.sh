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
# The monitors have CI jobs of their own; the exit-script jobs set these and skip them here.
if [ -n "${MASON_SKIP_COMPOSE:-}" ] || ! command -v gradle >/dev/null; then
    echo "== (Compose tests skipped: CI's monitor-compose job runs them)"
else
    echo "== Compose core and app tests"
    # Plain console output names a failing test; -q alone hid which one failed.
    (cd clients/compose && gradle test --console=plain > "${TMPDIR:-/tmp}/s79-gradle.log" 2>&1) \
      || { grep -E "FAILED|tests completed" "${TMPDIR:-/tmp}/s79-gradle.log" >&2; exit 1; }
fi
if [ -n "${MASON_SKIP_SWIFT:-}" ] || [ "$(uname)" != Darwin ] || ! command -v swift >/dev/null; then
    echo "== (Swift tests skipped: they need macOS; CI's monitor-swift job runs them)"
else
    echo "== Swift tests"
    (cd clients/swift && swift test --filter MonitorCoreTests)
fi
echo "s79 OK (the hand checks are in s79-manual.sh)"
