#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s752 exit test: the CountTextTest source exists and the stage note has a summary.
set -euo pipefail
cd "$(dirname "$0")/../.."
test -f clients/compose/app/src/test/kotlin/mason/monitor/ui/CountTextTest.kt
grep -q '^Summary: ' docs/stages/s752.md
