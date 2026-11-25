#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1086 exit test: s101 starts near the real exit-number ceiling (9999) and expects it.
set -euo pipefail
cd "$(dirname "$0")/../.."

grep -q '^Summary: ' docs/stages/s1086.md
grep -q 'MASON_ADHOC_EXIT_START=9960 ' scripts/exit/s101.sh
grep -q "'ceiling is 9999'" scripts/exit/s101.sh
echo "OK s1086"
