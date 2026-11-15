#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s329 exit test: the task detail copy buttons are labelled in both monitors.
set -euo pipefail
cd "$(dirname "$0")/.."/..
grep -q '"Copy worktree"' clients/swift/Sources/MonitorUI/Components.swift
grep -q '"Copy branch name"' clients/swift/Sources/MonitorUI/Components.swift
grep -q '"Copy worktree"' clients/compose/app/src/main/kotlin/mason/monitor/ui/Components.kt
grep -q '"Copy branch name"' clients/compose/app/src/main/kotlin/mason/monitor/ui/Components.kt
echo "s329 OK"
