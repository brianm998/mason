#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s328 exit test: both monitors have a test feeding a welcome with a known commit.
set -euo pipefail
cd "$(dirname "$0")/../.."
grep -q "homeShowsTheDaemonsShortCommit" clients/compose/core/src/test/kotlin/mason/monitor/MonitorCoreTest.kt
grep -q "testHomeShowsTheDaemonsShortCommit" clients/swift/Tests/MonitorCoreTests/MonitorCoreTests.swift
echo "s328 OK"
