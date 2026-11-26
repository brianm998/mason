#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1102 exit test: the exit scripts that went red on develop assert on the current design.
set -euo pipefail
cd "$(dirname "$0")/../.."
grep -q '^Summary: ' docs/stages/s1102.md
grep -q 'counts\[-1\]\["total"\] == 40' scripts/exit/s70.sh
! grep -q 'total_source"\] == "expected"' scripts/exit/s70.sh
bash -n scripts/exit/s70.sh
bash scripts/exit/s1068.sh
bash scripts/exit/s1026.sh
echo "s1102 passed"
