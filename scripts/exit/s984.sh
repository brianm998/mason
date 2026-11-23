#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s984 exit test: "failed" is a section page in both monitor clients, and the client tests are green.
set -euo pipefail
cd "$(dirname "$0")/../.."
grep -E 'sectionNames' -A2 clients/swift/Sources/MonitorCore/Navigation.swift | grep -q '"failed"'
grep -E 'sectionNames' -A2 clients/compose/core/src/main/kotlin/mason/monitor/Navigation.kt | grep -q '"failed"'
# The Swift package's test bundles do not all build on Linux (MasonMascotTests), so run it on macOS only.
if [ "$(uname)" = Darwin ] && command -v swift >/dev/null && [ -z "${MASON_SKIP_SWIFT:-}" ]; then
  swift test --package-path clients/swift --filter 'MonitorCoreTests|PhoneNavTests'
fi
if command -v java >/dev/null && command -v gradle >/dev/null && [ -z "${MASON_SKIP_COMPOSE:-}" ]; then
  (cd clients/compose && gradle --no-daemon -q :core:test)
fi
