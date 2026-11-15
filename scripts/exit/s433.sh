#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s433 exit test: both monitors have the Devices feature recorded, the QR encoder and the devices
# model pass their tests, and the conformance features check passes with the new row.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
grep -q '^\[feature.settings-devices\]' clients/features.toml || { echo "FAILED: no settings-devices row" >&2; exit 1; }
for f in clients/swift/Sources/MonitorUI/DevicesSection.swift clients/compose/app/src/main/kotlin/mason/monitor/ui/Devices.kt; do
  [ -f "$f" ] || { echo "FAILED: $f is missing" >&2; exit 1; }
done
(cd clients/compose && gradle --offline -q :core:test --tests 'mason.monitor.QrCodeTest' --tests 'mason.monitor.DevicesTest')
cargo run -q -p mason-conformance -- features clients/features.toml
echo "s433 OK"
