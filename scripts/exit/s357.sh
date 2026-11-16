#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s357 exit test: links in both monitors share one link look (colour, underline, hand cursor).
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test -q -p mason-conformance --lib tokens::
if grep -rn 'buttonStyle(.link)' clients/swift/Sources/MonitorUI; then
  echo "s357 FAIL: buttonStyle(.link) remains in MonitorUI" >&2
  exit 1
fi
if command -v swift >/dev/null 2>&1 && [ "$(uname)" = Darwin ]; then
  (cd clients/swift && swift test --filter LinkedTextTests)
fi
if [ -x clients/compose/gradlew ]; then
  (cd clients/compose && ./gradlew --offline -q :app:test --tests 'mason.monitor.ui.LinkedTextTest')
fi
echo "s357 OK"
