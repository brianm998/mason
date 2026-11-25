#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1068 exit test: Compose settle waits on applied views (a signal, decision 1026) and the Rust
# capturer reports its quiet exit.
set -euo pipefail
cd "$(dirname "$0")/../.."
t=clients/compose/app/src/main/kotlin/mason/monitor/Tool.kt
grep -q 'val covered = model.lastViewSeq >= seq' "$t"
! grep -q 'val covered = model.lastSeq' "$t"
grep -q 'AGREEING_ROUND_TRIPS' "$t"
grep -q 'timed out waiting for ' "$t"
grep -q '60_000' "$t"
grep -q 'settle: quiet exit, screen at' crates/conformance/src/main.rs
grep -q 'fn should_leave' crates/conformance/src/main.rs
grep -q '^Summary: ' docs/stages/s1068.md
grep -q '1019' docs/decisions/1057-monitor-compose-settle-counts-events.md
bash -n scripts/exit/s13.sh
echo "s1068 passed"
