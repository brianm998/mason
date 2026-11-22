#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Runs the exit scripts that this change adds or edits, or can break (s669), relative to its merge base with the base
# branch (s249), for the `exit-scripts` suite. The base comes from MASON_BASE (a test step's
# environment) and falls back to origin/develop (a run_check, which does not have it).
#
# Prints one line per script, `PASS sNNN` or `FAIL sNNN`, which the suite's regex parser reads, and
# keeps each failing script's output after its FAIL line. Exits non-zero if any failed. A change
# that adds or edits no runnable exit script prints `PASS none` (a deleted script has nothing to run).
# Scripts that need a real service, person or device (`sNN-real.sh`, `sNN-manual.sh`) are skipped,
# as scripts/run-exit-scripts.sh skips them.
set -uo pipefail
here=$(cd "$(dirname "$0")" && pwd)
cd "$(git rev-parse --show-toplevel)"
base_ref=${MASON_BASE:-origin/develop}
if [ -f .mason/changed-files.txt ]; then
  # A remote run: the checkout is shallow, so MASON listed the change's files (s759).
  base=
elif ! base=$(git merge-base "$base_ref" HEAD 2>/dev/null); then
  echo "FAIL no-merge-base-with-$base_ref"
  exit 1
fi
# The run_check's CPU reading is the machine's; a script that drives the gate sets its own file.
cpu=$(mktemp); echo 0 > "$cpu"; export MASON_CPU_FILE="$cpu"
# A changed repetition script (s80, s99, s169, ...) proves a test stable by repeating it; once is
# enough to prove the edit still runs, and the full count would outlast the step (s668).
export MASON_EXIT_REPEAT=${MASON_EXIT_REPEAT:-1}
log=$(mktemp)
trap 'rm -f "$cpu" "$log"' EXIT
fail=0; n=0
# run_limited SCRIPT SECONDS LOG: runs the script with its output in LOG; 124 when it was killed for time.
run_limited() {
  local s=$1 secs=$2 out=$3 pid watcher flag rc
  if ! [ "$secs" -gt 0 ] 2>/dev/null; then bash "$s" > "$out" 2>&1; return $?; fi
  flag=$(mktemp); rm -f "$flag"
  bash "$s" > "$out" 2>&1 & pid=$!
  ( sleep "$secs"; if kill -0 "$pid" 2>/dev/null; then : > "$flag"; pkill -P "$pid" 2>/dev/null; kill "$pid" 2>/dev/null; fi ) >/dev/null 2>&1 &
  watcher=$!
  wait "$pid"; rc=$?
  kill "$watcher" 2>/dev/null; wait "$watcher" 2>/dev/null
  if [ -e "$flag" ]; then rm -f "$flag"; return 124; fi
  return $rc
}
# Scripts the change adds or edits, plus those whose text names a changed path, crate or test target
# (scripts/select-exit-scripts.sh, s669).
# The selector is read from this helper'"'"'s own directory, so a helper run inside another repository still finds it (s716).
selected=$(bash "$here/select-exit-scripts.sh" "$base") || { echo "FAIL select-exit-scripts"; exit 1; }
# MASON_EXIT_SHARD=i/n (a GitHub shard, s843): keep only the scripts that scripts/run-exit-scripts.sh
# plans for shard i. Its plan covers the whole directory, so a script's shard does not move with the
# selection, and every selected script runs on exactly one shard.
if [ -n "${MASON_EXIT_SHARD:-}" ]; then
  mine=$(MASON_EXIT_PLAN=1 bash "$here/run-exit-scripts.sh") || { echo "FAIL exit-shard-plan"; exit 1; }
  keep=
  for s in $selected; do
    printf '%s\n' "$mine" | grep -qxF -- "$s" && keep="$keep $s"
  done
  selected=$keep
fi
# One script must not use a whole run: over the limit it is killed and reported, the rest still run (s843).
limit=${MASON_EXIT_SCRIPT_LIMIT_S:-900}
for s in $selected; do
  case "$s" in
    *-real.sh|*-manual.sh) continue ;;
  esac
  [ -e "$s" ] || continue
  id=$(basename "$s" .sh)
  n=$((n + 1))
  rc=0
  run_limited "$s" "$limit" "$log" || rc=$?
  if [ "$rc" -eq 0 ]; then
    echo "PASS $id"
  else
    if [ "$rc" -eq 124 ]; then echo "FAIL $id (over $limit s)"; else echo "FAIL $id"; fi
    tail -n 30 "$log" | sed 's/^/    | /'
    fail=1
  fi
done
if [ "$n" -eq 0 ]; then
  echo "PASS none"
fi
exit $fail
