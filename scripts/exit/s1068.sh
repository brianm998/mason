#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1068 exit test: Compose settle waits on views and names the view seq reached on timeout;
# both capturers wait for the exact seq in a demo (decision 1025).
set -euo pipefail
cd "$(dirname "$0")/../.."
t=clients/compose/app/src/main/kotlin/mason/monitor/Tool.kt
grep -qF 'val covered = model.lastViewSeq >= seq' "$t"
! grep -q 'val reached = model.lastSeq' "$t"
grep -qF 'Settled is a signal, not a quiet window' "$t"
grep -q 'val unmoved = !model.demo &&' "$t"
grep -q '!demo && views >= 3' crates/conformance/src/main.rs
grep -qF 'applied up to ' "$t"
grep -q '60_000' "$t"
grep -q '^Summary: ' docs/stages/s1068.md
grep -q '1019' docs/decisions/1057-monitor-compose-settle-counts-events.md
bash -n scripts/exit/s13.sh
echo "s1068 passed"
