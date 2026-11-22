#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s849 exit test: the exit-scripts job has at least 90 minutes, the full-run script limit is unchanged, s636 is weighted for its repeats (#792).
set -euo pipefail
cd "$(dirname "$0")/../.."
t=$(awk '/^  exit-scripts:/{j=1;next} j&&/^  [a-z]/{j=0} j&&/timeout-minutes:/{print $2; exit}' .github/workflows/ci.yml)
[ -n "$t" ] && [ "$t" -ge 90 ] || { echo "exit-scripts timeout is '$t'" >&2; exit 1; }
awk '/^  exit-scripts:/{j=1;next} j&&/^  [a-z]/{j=0} j&&/MASON_EXIT_SCRIPT_LIMIT_S/&&/workflow_dispatch/&&/3600/{f=1} END{exit !f}' .github/workflows/ci.yml \
    || { echo "full-run script limit is not 3600" >&2; exit 1; }
awk '$1=="s636"&&$2>=960{f=1} END{exit !f}' scripts/exit/weights.txt || { echo "s636 weight below 960" >&2; exit 1; }
! grep -vE '^s[0-9]+ [0-9]+$' scripts/exit/weights.txt | grep -q . || { echo "bad weight line" >&2; exit 1; }
echo "s849 OK"
