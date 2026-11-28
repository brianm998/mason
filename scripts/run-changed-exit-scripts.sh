#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Runs the exit scripts that this change adds or edits, or can break (s669), relative to its merge base with the base
# branch (s249), for the `exit-scripts` suite. The base comes from MASON_BASE (a test step's
# environment) and falls back to origin/develop (a run_check, which does not have it).
#
# Prints one line per script, `PASS sNNN` or `FAIL sNNN`, which the suite's regex parser reads, and
# keeps each failing script's output after its FAIL line. Exits non-zero if any failed. A change
# that adds or edits no runnable exit script prints `PASS none` (a deleted script has nothing to run).
#
# Time budget: MASON_RUN_BUDGET_S (set by the daemon to the run's limit, counted from when the
# command starts) is the time this run has. Before each script the runner checks what is left
# (budget, minus the time used, minus a margin of MASON_RUN_BUDGET_MARGIN_S, default 120); a script
# that starts gets at most that long; when less than MASON_RUN_BUDGET_FLOOR_S (default 60) is left,
# no more start, and each script not run is `FAIL sNNN (not run: out of time; ...)`, so the run ends
# red with the passes listed instead of being killed with no verdict.
#
# A script killed at the per-script limit (#1291): in an agent's check (MASON_EXIT_IN_CHECK=1, set
# by the daemon) it is `NOT RUN sNNN (over N s, too slow for a check)`, counted as not run, and
# does not make the run red. Anywhere else (a test step, a landing run) it is `FAIL sNNN (over N s)`.
# A script cut short because the run's time budget left it less than the limit is a FAIL either way.
#
# MASON_TESTS_ALREADY_RAN (set by the daemon when the cargo-workspace suite runs in the same check
# or test step, #1291) is read by the scripts themselves: they leave out their own cargo-test parts.
#
# Passes kept between runs: with MASON_EXIT_CACHE=1 (an agent's run_check sets it; a test step and a
# landing run never do, so a gate counts only what it ran itself) and MASON_TASK_CACHE set to an
# existing directory, each PASS is recorded under it, keyed by the script's path and
# content, MASON_EXIT_REPEAT, MASON_TESTS_ALREADY_RAN and the tree (HEAD, the uncommitted diff, untracked files). A later run
# on the same tree prints `PASS sNNN (cached)` for it. A FAIL is never recorded.
# Off this Mac (#1252) a node or GitHub runner has neither the directory nor the tree, so the
# coordinator keeps the passes: with MASON_EXIT_CACHE=1, MASON_EXIT_TREE_KEY (its key of the tree it
# sent) and MASON_EXIT_PASSES (the keys it kept, space-separated), a script whose key is listed
# prints `PASS sNNN (cached)`, and each script that passes here prints `MASON-EXIT-KEY <key>`, which
# the coordinator reads and removes from the output. The keys are made as above, from the given tree.
# Scripts that need a real service, person or device (`sNN-real.sh`, `sNN-manual.sh`) are skipped,
# as scripts/run-exit-scripts.sh skips them.
set -uo pipefail
# The budget counts from here, selection included.
SECONDS=0
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
# run_limited SCRIPT SECONDS LOG: runs the script with its output in LOG; 124, with killed=1, when it was
# killed for time (a script may itself exit 124).
run_limited() {
  local s=$1 secs=$2 out=$3 pid watcher flag rc
  killed=0
  if ! [ "$secs" -gt 0 ] 2>/dev/null; then bash "$s" > "$out" 2>&1; return $?; fi
  flag=$(mktemp); rm -f "$flag"
  bash "$s" > "$out" 2>&1 & pid=$!
  ( sleep "$secs"; if kill -0 "$pid" 2>/dev/null; then : > "$flag"; pkill -P "$pid" 2>/dev/null; kill "$pid" 2>/dev/null; fi ) >/dev/null 2>&1 &
  watcher=$!
  wait "$pid"; rc=$?
  kill "$watcher" 2>/dev/null; wait "$watcher" 2>/dev/null
  if [ -e "$flag" ]; then rm -f "$flag"; killed=1; return 124; fi
  return $rc
}
# Scripts the change adds or edits, plus those whose text names a changed path, crate or test target
# (scripts/select-exit-scripts.sh, s669).
# The selector is read from this helper'"'"'s own directory, so a helper run inside another repository still finds it (s716).
if [ -n "${MASON_EXIT_ONLY:-}" ]; then
  # MASON_EXIT_ONLY="scripts/exit/sA.sh scripts/exit/sB.sh": a check of a task whose testing the
  # owner skipped runs only the scripts the change adds or edits, not every script that names a
  # changed path (the selector picked about 160 for a one-script change on 2026-11-27).
  selected=
  for s in $MASON_EXIT_ONLY; do [ -e "$s" ] && selected="$selected $s"; done
else
  selected=$(bash "$here/select-exit-scripts.sh" "$base") || { echo "FAIL select-exit-scripts"; exit 1; }
fi
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
# Read here and not passed on: a script that drives this runner itself is not in the agent's check.
in_check=${MASON_EXIT_IN_CHECK:-}
unset MASON_EXIT_IN_CHECK
# One script must not use a whole run: over the limit it is killed and reported, the rest still run (s843).
limit=${MASON_EXIT_SCRIPT_LIMIT_S:-900}
budget=${MASON_RUN_BUDGET_S:-}
margin=${MASON_RUN_BUDGET_MARGIN_S:-120}
floor=${MASON_RUN_BUDGET_FLOOR_S:-60}
case "$budget" in ''|*[!0-9]*) budget= ;; esac
case "$margin" in ''|*[!0-9]*) margin=120 ;; esac
case "$floor" in ''|*[!0-9]*) floor=60 ;; esac
# hash_stdin: the SHA-256 of stdin (macOS shasum, else GNU sha256sum).
hash_stdin() {
  if command -v shasum >/dev/null 2>&1; then shasum -a 256 | cut -d' ' -f1; else sha256sum | cut -d' ' -f1; fi
}
# The cache: only for an existing directory MASON gave this task, and only in a real checkout.
cache=
tree_key=
if [ "${MASON_EXIT_CACHE:-}" = 1 ] && [ -n "${MASON_TASK_CACHE:-}" ] && [ -d "$MASON_TASK_CACHE" ] && [ ! -f .mason/changed-files.txt ]; then
  tree_key=$(
    {
      git rev-parse HEAD 2>/dev/null
      git diff HEAD --binary 2>/dev/null
      git ls-files -o --exclude-standard 2>/dev/null | while IFS= read -r f; do
        printf '%s\n' "$f"
        if [ -f "$f" ]; then hash_stdin < "$f"; fi
      done
    } | hash_stdin
  ) || tree_key=
  if [ -n "$tree_key" ] && mkdir -p "$MASON_TASK_CACHE/exit-scripts-pass" 2>/dev/null; then
    cache="$MASON_TASK_CACHE/exit-scripts-pass"
  fi
fi
remote_passes=
if [ "${MASON_EXIT_CACHE:-}" = 1 ] && [ -n "${MASON_EXIT_TREE_KEY:-}" ] && [ -z "$cache" ]; then
  tree_key=$MASON_EXIT_TREE_KEY
  remote_passes=" ${MASON_EXIT_PASSES:-} "
fi
cache_file() { # cache_file SCRIPT: the record that this script passed on this tree
  # A pass without the cargo parts is not a pass for a run that runs them (#1291).
  printf '%s\n' "$1" "$(hash_stdin < "$1")" "$tree_key" "${MASON_EXIT_REPEAT:-}" "${MASON_TESTS_ALREADY_RAN:+tests-ran}" | hash_stdin
}
# The cache settings are this run's own. A script that drives this runner itself (s759) must not
# take them, or its inner run prints `MASON-EXIT-KEY` lines too (#1291).
unset MASON_EXIT_CACHE MASON_EXIT_TREE_KEY MASON_EXIT_PASSES
passed=0; failed=0; notrun=0; out_of_time=0
left=
for s in $selected; do
  case "$s" in
    *-real.sh|*-manual.sh) continue ;;
  esac
  [ -e "$s" ] || continue
  id=$(basename "$s" .sh)
  n=$((n + 1))
  ckey=
  rkey=
  if [ -n "$remote_passes" ]; then
    rkey=$(cache_file "$s")
    case "$remote_passes" in
      *" $rkey "*)
        echo "PASS $id (cached)"
        passed=$((passed + 1))
        continue ;;
    esac
  fi
  if [ -n "$cache" ]; then
    ckey="$cache/$(cache_file "$s")"
    if [ -e "$ckey" ]; then
      echo "PASS $id (cached)"
      passed=$((passed + 1))
      continue
    fi
  fi
  this=$limit
  if [ -n "$budget" ]; then
    left=$((budget - SECONDS - margin))
    if [ "$left" -lt "$floor" ]; then out_of_time=1; fi
    if [ "$out_of_time" -eq 1 ]; then
      echo "FAIL $id (not run: out of time; run the check again to continue)"
      notrun=$((notrun + 1))
      fail=1
      continue
    fi
    # The script gets at most what is left, so this runner ends before the daemon's kill.
    if ! [ "$this" -gt 0 ] 2>/dev/null || [ "$left" -lt "$this" ]; then this=$left; fi
  fi
  rc=0
  run_limited "$s" "$this" "$log" || rc=$?
  if [ "$rc" -eq 0 ]; then
    echo "PASS $id"
    passed=$((passed + 1))
    if [ -n "$ckey" ]; then : > "$ckey" 2>/dev/null || true; fi
    if [ -n "$rkey" ]; then echo "MASON-EXIT-KEY $rkey"; fi
  else
    if [ "$killed" -eq 1 ] && [ "$in_check" = 1 ] && [ "$this" = "$limit" ]; then
      echo "NOT RUN $id (over $this s, too slow for a check)"
      tail -n 30 "$log" | sed 's/^/    | /'
      notrun=$((notrun + 1))
      continue
    fi
    if [ "$rc" -eq 124 ]; then echo "FAIL $id (over $this s)"; else echo "FAIL $id"; fi
    tail -n 30 "$log" | sed 's/^/    | /'
    failed=$((failed + 1))
    fail=1
  fi
done
if [ "$n" -eq 0 ]; then
  echo "PASS none"
fi
if [ -n "$budget" ]; then
  echo "exit-scripts: $passed passed, $failed failed, $notrun not run (time budget $budget s)"
fi
exit $fail
