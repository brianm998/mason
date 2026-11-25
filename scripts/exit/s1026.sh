#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1026 exit test: the evidence hooks for the Compose conformance red are in place.
set -euo pipefail
cd "$(dirname "$0")/../.."
grep -q '^Summary: ' docs/stages/s1026.md
d=docs/decisions/1019-monitor-compose-ubuntu-red-conformance-run-45005421555.md
for n in 0905 0707 0690; do grep -q "$n" "$d"; done
grep -q 'only in compose' scripts/exit/s13.sh
grep -q 'conformance" check --expected "$root/expected" --actual "$root/compose"' scripts/exit/s13.sh
# s1100: settle() waits on a signal (decision 1026); its timeout names the view seq reached.
grep -qF 'Settled is a signal, not a quiet window' clients/compose/app/src/main/kotlin/mason/monitor/Tool.kt
grep -qF 'applied up to ' clients/compose/app/src/main/kotlin/mason/monitor/Tool.kt
bash -n scripts/exit/s13.sh
echo "s1026 passed"
