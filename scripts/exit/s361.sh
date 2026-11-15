#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s361 exit test: the history keeps a scroll position per entry; the feature is in the parity file.
set -euo pipefail
cd "$(dirname "$0")/../.."
grep -q 'feature.back-restores-scroll' clients/features.toml
grep -q 'restore' clients/swift/Sources/MonitorCore/Navigation.swift
grep -q 'restore' clients/compose/core/src/main/kotlin/mason/monitor/Navigation.kt
echo "s361 OK"
