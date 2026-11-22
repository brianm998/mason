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
# Scripts the change adds or edits, plus those whose text names a changed path, crate or test target
# (scripts/select-exit-scripts.sh, s669).
# The selector is read from this helper'"'"'s own directory, so a helper run inside another repository still finds it (s716).
selected=$(bash "$here/select-exit-scripts.sh" "$base") || { echo "FAIL select-exit-scripts"; exit 1; }
# MASON_EXIT_SHARD=i/n (a sharded remote run sets it from MASON_SHARD and MASON_SHARDS, #873): run only
# shard i of the n, the selected scripts assigned heaviest first onto the lightest shard by
# scripts/exit/weights.txt (an unknown script weighs MASON_EXIT_DEFAULT_WEIGHT, 60), as
# scripts/run-exit-scripts.sh assigns them. Every selected script lands on exactly one shard.
shard=${MASON_EXIT_SHARD:-1/1}; si=${shard%/*}; sn=${shard#*/}
if [ "$sn" -gt 1 ] 2>/dev/null && [ -n "$selected" ]; then
  weights=${MASON_EXIT_WEIGHTS:-scripts/exit/weights.txt}
  selected=$(printf '%s\n' $selected | awk -v si="$si" -v sn="$sn" -v wf="$weights" -v dw="${MASON_EXIT_DEFAULT_WEIGHT:-60}" '
    BEGIN { while ((getline line < wf) > 0) { split(line, f, " "); if (f[1] ~ /^s[0-9]+$/ && f[2] ~ /^[0-9]+$/) w[f[1]] = f[2] + 0 } }
    { n++; path[n] = $0; name = $0; sub(/.*\//, "", name); sub(/\.sh$/, "", name)
      wt[n] = (name in w) ? w[name] : dw + 0 }
    END {
      for (k = 1; k <= sn; k++) tot[k] = 0
      for (a = 1; a <= n; a++) idx[a] = a
      for (a = 2; a <= n; a++) { v = idx[a]; b = a - 1
        while (b >= 1 && wt[idx[b]] < wt[v]) { idx[b + 1] = idx[b]; b-- }
        idx[b + 1] = v }
      for (a = 1; a <= n; a++) { i = idx[a]; best = 1
        for (k = 2; k <= sn; k++) if (tot[k] < tot[best]) best = k
        tot[best] += wt[i]; owner[i] = best }
      for (i = 1; i <= n; i++) if (owner[i] == si) print path[i]
    }')
fi
for s in $selected; do
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
