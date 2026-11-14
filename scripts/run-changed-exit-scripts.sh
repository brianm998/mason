#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Runs the exit scripts that this change adds or edits relative to its merge base with the base
# branch (s249), for the `exit-scripts` suite. The base comes from MASON_BASE (a test step's
# environment) and falls back to origin/develop (a run_check, which does not have it).
#
# Prints one line per script, `PASS sNNN` or `FAIL sNNN`, which the suite's regex parser reads, and
# keeps each failing script's output after its FAIL line. Exits non-zero if any failed. A change
# that adds or edits no runnable exit script prints `PASS none` (a deleted script has nothing to run).
# Scripts that need a real service, person or device (`sNN-real.sh`, `sNN-manual.sh`) are skipped,
# as scripts/run-exit-scripts.sh skips them.
set -uo pipefail
cd "$(git rev-parse --show-toplevel)"
base_ref=${MASON_BASE:-origin/develop}
if ! base=$(git merge-base "$base_ref" HEAD 2>/dev/null); then
  echo "FAIL no-merge-base-with-$base_ref"
  exit 1
fi
# The run_check's CPU reading is the machine's; a script that drives the gate sets its own file.
cpu=$(mktemp); echo 0 > "$cpu"; export MASON_CPU_FILE="$cpu"
log=$(mktemp)
trap 'rm -f "$cpu" "$log"' EXIT
fail=0; n=0
for s in $(git diff --name-only --diff-filter=AM "$base" HEAD -- 'scripts/exit/*.sh' | sort); do
  case "$s" in
    *-real.sh|*-manual.sh) continue ;;
  esac
  [ -e "$s" ] || continue
  id=$(basename "$s" .sh)
  n=$((n + 1))
  if bash "$s" > "$log" 2>&1; then
    echo "PASS $id"
  else
    echo "FAIL $id"
    tail -n 30 "$log" | sed 's/^/    | /'
    fail=1
  fi
done
if [ "$n" -eq 0 ]; then
  echo "PASS none"
fi
exit $fail
