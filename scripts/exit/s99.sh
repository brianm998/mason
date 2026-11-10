#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s99 exit test: the self-update test keeps its agent's step host, run repeatedly.
# Runs `cargo test -p mason-daemon --test update` N times (default 20) in a row
# and says how many runs were green out of how many were attempted.
set -uo pipefail
cd "$(dirname "$0")/../.."
n=${S99_RUNS:-20}
green=0
out=$(mktemp /tmp/s99.XXXXXX)
trap 'rm -f "$out"' EXIT
for i in $(seq 1 "$n"); do
  if cargo test -q -p mason-daemon --test update >"$out" 2>&1; then
    green=$((green + 1))
    echo "run $i: green"
  else
    echo "run $i: RED: $(grep -E '^test .* FAILED$' "$out" | tr '\n' ' ')"
    grep -E 'panicked|left:|right:|^host ' -A1 "$out" | head -20
  fi
done
echo "s99: $green green out of $n attempted"
[ "$green" -eq "$n" ]
