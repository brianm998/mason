#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s651 exit test: the ten-compiles wait-view test passes N times in a row (default 50).
set -uo pipefail
cd "$(dirname "$0")/../.."
n=${S651_RUNS:-50}
green=0
for i in $(seq 1 "$n"); do
  if cargo test -q -p mason-daemon --lib ten_compiles_behind_a_waiting_exclusive_lease_all_name_it >/dev/null 2>&1; then
    green=$((green + 1))
  else
    echo "run $i: RED"
  fi
done
echo "s651: $green green out of $n attempted"
[ "$green" -eq "$n" ]
