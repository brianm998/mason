#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1068 exit test: Compose settle waits on views and both capturers wait for the exact seq.
set -euo pipefail
cd "$(dirname "$0")/../.."
t=clients/compose/app/src/main/kotlin/mason/monitor/Tool.kt
grep -q 'val reached = model.lastViewSeq >= target' "$t"
! grep -q 'val reached = model.lastSeq' "$t"
grep -q 'val unmoved = !model.demo &&' "$t"
grep -q '!demo && screen.views.len() >= 3' crates/conformance/src/main.rs
grep -q 'timed out waiting for' "$t"
grep -q '60_000' "$t"
grep -q '^Summary: ' docs/stages/s1068.md
grep -q '1019' docs/decisions/1057-monitor-compose-settle-counts-events.md
bash -n scripts/exit/s13.sh
echo "s1068 passed"
