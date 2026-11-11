#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s147 exit test: the sidebar layout test exists. The visual check is in s147-manual.sh.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
grep -q "testRowAndHeaderFitAtTheFloorWidth" clients/swift/Tests/MonitorUITests/SidebarFitTests.swift
echo "s147 ok"
