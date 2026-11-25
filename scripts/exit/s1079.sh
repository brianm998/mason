#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1079 exit test: without timeout(1), a passing script returns the runner at once and leaves no sleep behind.
set -euo pipefail
cd "$(dirname "$0")/../.."
unset MASON_EXIT_SHARD

d=$(mktemp -d); trap 'rm -rf "$d"' EXIT
mkdir "$d/bin" "$d/scripts"
for t in bash sed awk grep head tail tr cut basename dirname mktemp sleep pkill pgrep kill tee rm cat wc git env sort uniq date; do
  p=$(command -v "$t" 2>/dev/null || true)
  case $p in /*) ln -s "$p" "$d/bin/$t" ;; esac
done
echo 'echo PASSING-MARK' > "$d/scripts/s21.sh"
printf 's21 1\n' > "$d/weights.txt"
PATH="$d/bin" MASON_EXIT_DIR="$d/scripts" MASON_EXIT_WEIGHTS="$d/weights.txt" MASON_EXIT_SCRIPT_LIMIT_S=977 \
  bash scripts/run-exit-scripts.sh > "$d/out.txt" 2>&1 || { cat "$d/out.txt"; echo "runner failed"; exit 1; }
grep -q PASSING-MARK "$d/out.txt" || { cat "$d/out.txt"; echo "script did not run"; exit 1; }
if pgrep -f 'sleep 977' >/dev/null; then pkill -f 'sleep 977' || true; echo "watcher sleep outlived the script"; exit 1; fi
echo "s1079 passed"
