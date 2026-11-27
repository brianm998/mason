#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1237 exit test: the daemon's apply answer carries the `true` test warning in `warnings`, and both
# monitors' parsers read plan warnings, apply warnings and notes (decision 1227).
set -euo pipefail
cd "$(dirname "$0")/../.."

grep -q '^Summary: ' docs/stages/s1237.md
cargo test -q -p mason-onboard apply_returns_the_true_test_command_warning_in_warnings
grep -q 'o\["warnings"\]' clients/swift/Sources/MonitorCore/AddProject.swift
grep -q 'o\["notes"\]' clients/swift/Sources/MonitorCore/AddProject.swift
grep -q 'optJSONArray("warnings")' clients/compose/core/src/main/kotlin/mason/monitor/AddProject.kt
grep -q 'optJSONArray("notes")' clients/compose/core/src/main/kotlin/mason/monitor/AddProject.kt
echo "OK s1237"
