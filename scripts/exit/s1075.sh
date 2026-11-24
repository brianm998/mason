#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1075 exit test: the runner's no-timeout(1) fallback returns when a script ends and
# kills a script's whole process group at the limit.
set -euo pipefail
cd "$(dirname "$0")/../.."
if [ -z "${S1075_GUARDED:-}" ]; then S1075_GUARDED=1 exec perl -e 'alarm 120; exec @ARGV' bash "$0"; fi
unset MASON_EXIT_SHARD

d=$(mktemp -d); trap 'rm -rf "$d"' EXIT
cat > "$d/s21.sh" <<'X'
echo PASSING-MARK
X
cat > "$d/s22.sh" <<'X'
sleep 7731 &
sleep 7732
X
printf 's21 1\ns22 1\n' > "$d/weights.txt"

# (a) a passing script returns at once and leaves no watcher sleep.
out=$(MASON_EXIT_FALLBACK=1 MASON_EXIT_DIR="$d" MASON_EXIT_WEIGHTS="$d/weights.txt" \
  MASON_EXIT_SCRIPT_LIMIT_S=7733 MASON_EXIT_PLAN= bash -c 'mv "$MASON_EXIT_DIR/s22.sh" "$MASON_EXIT_DIR/x22"; bash scripts/run-exit-scripts.sh 2>&1; mv "$MASON_EXIT_DIR/x22" "$MASON_EXIT_DIR/s22.sh"') \
  || { echo "passing run failed"; echo "$out"; exit 1; }
echo "$out"
echo "$out" | grep -q '^s21: [0-9]* s' || { echo "no s21 timing line"; exit 1; }
! pgrep -f 'sleep 7733' >/dev/null || { echo "watcher sleep left"; exit 1; }

# (b) a hanging script with a descendant holding stdout is killed and the run ends.
rm "$d/s21.sh"
out=$(MASON_EXIT_FALLBACK=1 MASON_EXIT_DIR="$d" MASON_EXIT_WEIGHTS="$d/weights.txt" \
  MASON_EXIT_SCRIPT_LIMIT_S=2 bash scripts/run-exit-scripts.sh 2>&1) && { echo "should have failed"; exit 1; }
echo "$out"
echo "$out" | grep -q 'FAILED: scripts/exit/s22.sh\|FAILED: .*s22.sh (over 2 s)' || { echo "limit not reported"; exit 1; }
! pgrep -f 'sleep 773[12]' >/dev/null || { echo "descendant left"; exit 1; }
echo "s1075 passed"
