#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1104 exit test: Compose settle is signal-based and reports a quiet exit with views behind.
set -euo pipefail
cd "$(dirname "$0")/../.."
t=clients/compose/app/src/main/kotlin/mason/monitor/Tool.kt
grep -q '^Summary: ' docs/stages/s1104.md
grep -q 'covered = model.lastViewSeq >= seq' "$t"
grep -q 'settle: quiet exit, views at' "$t"
for f in "$t" scripts/exit/s1068.sh scripts/exit/s1026.sh; do
  ! grep -q 'QUIET_TICKS' "$f" || { echo "QUIET_TICKS in $f" >&2; exit 1; }
done
bash scripts/exit/s1068.sh
bash scripts/exit/s1026.sh
echo "s1104 passed"
