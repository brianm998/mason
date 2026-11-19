#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s629 exit test: the exit-script guards pass and the s469 real check is listed.
set -euo pipefail
cd "$(dirname "$0")/../.."
bash scripts/check-exit-guards.sh
grep -q s469-real.sh docs/real-checks.md
echo "s629 ok"
