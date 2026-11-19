#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s641 exit test: s597 no longer trips s576's stale-target guard.
set -euo pipefail
cd "$(dirname "$0")/../.."
grep -q '^Summary: ' docs/stages/s641.md
! grep -rnE --exclude=s576.sh -- '--test (limits|stop)\b' scripts/exit
! grep -qF -- "--test lim""its" scripts/exit/s597.sh
bash scripts/exit/s597.sh
echo "s641: ok"
