#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1208 exit test: s1007 (only the test step can be skipped) passes five times in a row,
# so a skip of a queued task is judged by the project's loaded pipeline, not the built-in one.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
for i in 1 2 3 4 5; do
  echo "== s1007 run $i of 5"
  scripts/exit/s1007.sh || { echo "FAILED: s1007 run $i" >&2; exit 1; }
done
echo "s1208 OK"
