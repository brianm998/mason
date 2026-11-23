#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1038 exit test: a failed exit script's last lines and its limit kill are named in the log.
set -euo pipefail
cd "$(dirname "$0")/../.."

d=$(mktemp -d); trap 'rm -rf "$d"' EXIT
cat > "$d/s1.sh" <<'X'
echo PASSING-OUTPUT-MARK
X
cat > "$d/s2.sh" <<'X'
echo FAILING-REASON-MARK
exit 3
X
cat > "$d/s3.sh" <<'X'
echo HANGING-MARK
sleep 30
X
printf 's1 1\ns2 1\ns3 1\n' > "$d/weights.txt"
out=$(MASON_EXIT_DIR="$d" MASON_EXIT_WEIGHTS="$d/weights.txt" MASON_EXIT_SCRIPT_LIMIT_S=2 \
  bash scripts/run-exit-scripts.sh 2>&1) && { echo "runner should have failed"; exit 1; }
echo "$out"
fl=$(echo "$out" | grep '^FAILED scripts:')
echo "$fl" | grep -q 's2.sh' || { echo "s2 not listed"; exit 1; }
echo "$fl" | grep -q 's3.sh (over 2 s)' || { echo "limit marker missing"; exit 1; }
echo "$out" | grep -q -- '-- last 40 lines of s2' || { echo "tail missing"; exit 1; }
[ "$(echo "$out" | grep -c FAILING-REASON-MARK)" -ge 2 ] || { echo "failing output not repeated"; exit 1; }
[ "$(echo "$out" | grep -c PASSING-OUTPUT-MARK)" -eq 1 ] || { echo "passing output repeated"; exit 1; }
echo "s1038 passed"
