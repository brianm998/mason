#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s876 exit test: Settings shows and changes where runs go, in both monitors (#891).
# Static checks first (the feature entry, both models name /remote), then the Swift and Compose
# tests by name (when swift / gradle are installed; MASON_SKIP_SWIFT or MASON_SKIP_COMPOSE skips one).
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
grep -q '^\[feature\.settings-placement\]' clients/features.toml || { echo "FAIL: no [feature.settings-placement]" >&2; exit 1; }
grep -q '"/remote"' clients/swift/Sources/MonitorCore/Model.swift || { echo "FAIL: the Swift model does not use /remote" >&2; exit 1; }
grep -q '"/remote"' clients/compose/core/src/main/kotlin/mason/monitor/Model.kt || { echo "FAIL: the Compose model does not use /remote" >&2; exit 1; }
grep -q 'RemoteSettings' clients/swift/Sources/MonitorCore/SettingsModel.swift || { echo "FAIL: no Swift RemoteSettings" >&2; exit 1; }
grep -q 'RemoteSettings' clients/compose/core/src/main/kotlin/mason/monitor/SettingsModel.kt || { echo "FAIL: no Compose RemoteSettings" >&2; exit 1; }
if [ "$(uname)" = Darwin ] && command -v swift >/dev/null && [ -z "${MASON_SKIP_SWIFT:-}" ]; then
  (cd clients/swift && swift test --filter SettingsModelTests && swift test --filter RemotePlacementTests)
fi
if command -v java >/dev/null && command -v gradle >/dev/null && [ -z "${MASON_SKIP_COMPOSE:-}" ]; then
  (cd clients/compose && gradle --console=plain :core:test --tests '*SettingsModelTest' :app:test --tests '*RemotePlacementTest')
fi
echo "s876 passed"
