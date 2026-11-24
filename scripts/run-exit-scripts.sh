#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Runs every scripts/exit/sNN.sh that needs no real service, person or device.
# Scripts needing those are named sNN-real.sh or sNN-manual.sh and are skipped.
#
# Environment:
#   MASON_EXIT_SHARD=i/n          run only shard i of n. Scripts are assigned by weight
#                                 (scripts/exit/weights.txt, seconds per script), heaviest
#                                 first onto the lightest shard, so membership does not
#                                 move when a script is added. Unknown scripts weigh
#                                 MASON_EXIT_DEFAULT_WEIGHT (60).
#   MASON_EXIT_SCRIPT_LIMIT_S=N   kill a script after N seconds and report it as
#                                 `FAILED: scripts/exit/sNN.sh (over N s)`. 0 or unset: no limit.
#   MASON_EXIT_PLAN=1             print the scripts of the shard and exit.
#   MASON_EXIT_DIR, MASON_EXIT_WEIGHTS   other script directory / weights file (tests).
#   MASON_EXIT_REPEAT=N           read by the repetition scripts (s80, s99, s169, s170, s482,
#                                 s497): how many times each runs its cargo test.
set -uo pipefail
cd "$(git rev-parse --show-toplevel)"
dir=${MASON_EXIT_DIR:-scripts/exit}
weights=${MASON_EXIT_WEIGHTS:-$dir/weights.txt}
limit=${MASON_EXIT_SCRIPT_LIMIT_S:-0}
shard=${MASON_EXIT_SHARD:-1/1}; si=${shard%/*}; sn=${shard#*/}

# The scripts of this shard, one path per line, in name order.
plan() {
  local list=()
  local s
  for s in "$dir"/s[0-9][0-9].sh "$dir"/s[0-9][0-9][0-9].sh "$dir"/s[0-9][0-9][0-9][0-9].sh; do
    [ -e "$s" ] && list+=("$s")
  done
  [ ${#list[@]} -gt 0 ] || return 0
  printf '%s\n' "${list[@]}" | awk -v si="$si" -v sn="$sn" -v wf="$weights" -v dw="${MASON_EXIT_DEFAULT_WEIGHT:-60}" '
    BEGIN { while ((getline line < wf) > 0) { split(line, f, " "); if (f[1] ~ /^s[0-9]+$/ && f[2] ~ /^[0-9]+$/) w[f[1]] = f[2] + 0 } }
    { n++; path[n] = $0; name = $0; sub(/.*\//, "", name); sub(/\.sh$/, "", name)
      wt[n] = (name in w) ? w[name] : dw + 0 }
    END {
      for (k = 1; k <= sn; k++) tot[k] = 0
      # heaviest first; equal weights in name order (the input order)
      for (a = 1; a <= n; a++) idx[a] = a
      for (a = 2; a <= n; a++) { v = idx[a]; b = a - 1
        while (b >= 1 && wt[idx[b]] < wt[v]) { idx[b + 1] = idx[b]; b-- }
        idx[b + 1] = v }
      for (a = 1; a <= n; a++) { i = idx[a]; best = 1
        for (k = 2; k <= sn; k++) if (tot[k] < tot[best]) best = k
        tot[best] += wt[i]; owner[i] = best }
      for (i = 1; i <= n; i++) if (owner[i] == si) print path[i]
    }'
}

if [ -n "${MASON_EXIT_PLAN:-}" ]; then plan; exit 0; fi

# A script's daemon must not act on what else this machine is doing, and a real
# CPU reading would also replace the compile counts the scripts set up. A script
# that drives the gate itself sets its own file.
cpu=$(mktemp); echo 0 > "$cpu"; export MASON_CPU_FILE="$cpu"
trap 'rm -f "$cpu"' EXIT

# Runs one script under the limit. Returns 0 pass, 1 fail, 2 over the limit.
run_limited() {
  local s=$1 rc
  if [ "$limit" -le 0 ] 2>/dev/null; then bash "$s"; [ $? -eq 0 ] && return 0 || return 1; fi
  local t=""
  if command -v timeout >/dev/null; then t=timeout; elif command -v gtimeout >/dev/null; then t=gtimeout; fi
  if [ -n "$t" ]; then
    "$t" -k 10 "$limit" bash "$s"; rc=$?
    [ $rc -eq 0 ] && return 0
    { [ $rc -eq 124 ] || [ $rc -eq 137 ]; } && return 2
    return 1
  fi
  # No timeout(1) (macOS): a watcher kills the script and its children.
  local flag; flag=$(mktemp); rm -f "$flag"
  bash "$s" & local pid=$!
  ( sleep "$limit"; if kill -0 "$pid" 2>/dev/null; then : > "$flag"; pkill -P "$pid" 2>/dev/null; kill "$pid" 2>/dev/null; fi ) &
  local watcher=$!
  wait "$pid"; rc=$?
  kill "$watcher" 2>/dev/null; wait "$watcher" 2>/dev/null
  if [ -e "$flag" ]; then rm -f "$flag"; return 2; fi
  [ $rc -eq 0 ] && return 0 || return 1
}

# The first panic (with its message line) or failing line of a script's log, for the summary.
cause_of() {
  local clean; clean=$(mktemp)
  sed $'s/\x1b\\[[0-9;]*[A-Za-z]//g' "$1" > "$clean"
  local n c=""
  n=$(grep -naE 'panicked at|FAIL|[Ee]rror|not ok' "$clean" | head -n 1 | cut -d: -f1)
  if [ -n "$n" ]; then
    c=$(sed -n "${n}p" "$clean")
    case $c in *"panicked at"*) c="$c $(sed -n "$((n + 1))p" "$clean")" ;; esac
  else
    c=$(grep -av '^[[:space:]]*$' "$clean" | tail -n 1)
    if [ -n "$c" ]; then c="last output: $c"; else c="no output"; fi
  fi
  rm -f "$clean"
  printf '%s' "$c" | tr '\r\n' '  ' | sed 's/[[:space:]][[:space:]]*/ /g' | cut -c1-200
}

fail=0; failed=""; causes=""
while IFS= read -r -u 3 s; do
  [ -n "$s" ] || continue
  name=$(basename "$s" .sh)
  echo "== $s"
  start=$SECONDS
  log=$(mktemp)
  run_limited "$s" 2>&1 | tee "$log"; rc=${PIPESTATUS[0]}
  echo "$name: $((SECONDS - start)) s"
  case $rc in
    0) ;;
    2) echo "FAILED: $s (over $limit s)" >&2; fail=1; failed="$failed $s (over $limit s)"
       echo "-- last 40 lines of $name" >&2; tail -n 40 "$log" >&2 ;;
    *) echo "FAILED: $s" >&2; fail=1; failed="$failed $s"
       echo "-- last 40 lines of $name" >&2; tail -n 40 "$log" >&2 ;;
  esac
  [ "$rc" -eq 0 ] || causes="$causes  cause $name: $(cause_of "$log")"$'\n'
  rm -f "$log"
done 3< <(plan)
[ "$fail" -eq 0 ] || printf '%s' "$causes" >&2
[ "$fail" -eq 0 ] || echo "FAILED scripts:$failed" >&2
exit $fail
