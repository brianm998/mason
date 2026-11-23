#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1045 exit test: a red run repeats each failed script's cause just before the final FAILED line.
set -euo pipefail
cd "$(dirname "$0")/../.."
unset MASON_EXIT_SHARD  # the scratch runs must cover all fake scripts, not the CI shard

d=$(mktemp -d); trap 'rm -rf "$d"' EXIT
cat > "$d/s21.sh" <<'X'
echo fine
X
cat > "$d/s22.sh" <<'X'
printf 'noise\n\033[31mthread '"'x'"' panicked at x.rs:1:1:\033[0m\nboom-message\nmore\n'
exit 1
X
cat > "$d/s23.sh" <<'X'
echo 'FAIL s23: wrong'
echo trailing
exit 1
X
printf 's21 1\ns22 1\ns23 1\n' > "$d/weights.txt"
out=$(MASON_EXIT_DIR="$d" MASON_EXIT_WEIGHTS="$d/weights.txt" \
  bash scripts/run-exit-scripts.sh 2>&1) && { echo "runner should have failed"; exit 1; }
echo "$out"
echo "$out" | grep -q '^  cause s22: .*panicked at x.rs:1:1: boom-message' || { echo "s22 cause missing"; exit 1; }
echo "$out" | grep -q '^  cause s23: FAIL s23: wrong$' || { echo "s23 cause missing"; exit 1; }
echo "$out" | grep -q 'cause s21' && { echo "passing script has a cause"; exit 1; }
[ "$(echo "$out" | tail -n 1)" = "FAILED scripts: $d/s22.sh $d/s23.sh" ] || { echo "last line wrong"; exit 1; }
echo "$out" | tail -n 2 | head -n 1 | grep -q '^  cause s23' || { echo "causes not before last line"; exit 1; }

printf 'true\n' > "$d/s22.sh"; printf 'true\n' > "$d/s23.sh"
out=$(MASON_EXIT_DIR="$d" MASON_EXIT_WEIGHTS="$d/weights.txt" bash scripts/run-exit-scripts.sh 2>&1)
echo "$out" | grep -qE 'cause|FAILED' && { echo "green run printed failure text"; exit 1; }
echo "s1045 passed"
