#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s668 exit test: CI's exit-script shards fit their limits.
#  1. The runner reports a script over its limit as `FAILED: ... (over N s)`, a failing script
#     as failed, and exits non-zero; a passing one is not reported.
#  2. Shards are assigned by weight, and unknown scripts get a default weight.
#  3. The repetition scripts honour MASON_EXIT_REPEAT (counted through a cargo shim).
#  4. ci.yml sets MASON_EXIT_REPEAT and MASON_EXIT_SCRIPT_LIMIT_S on the exit-scripts job.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
tmp=$(mktemp -d /tmp/mason-s668.XXXXXX); trap 'rm -rf "$tmp"' EXIT
die() { echo "s668 FAILED: $*" >&2; exit 1; }
# CI sets these for the job's own shard; the fixtures below must not inherit them.
unset MASON_EXIT_SHARD MASON_EXIT_PLAN MASON_EXIT_REPEAT MASON_EXIT_DEFAULT_WEIGHT MASON_EXIT_WEIGHTS

# 1. over-limit, failing and passing fake scripts
d1=$tmp/one; mkdir "$d1"
printf '#!/usr/bin/env bash\nsleep 30\n' > "$d1/s11.sh"
printf '#!/usr/bin/env bash\nexit 3\n' > "$d1/s12.sh"
printf '#!/usr/bin/env bash\nexit 0\n' > "$d1/s13.sh"
rc=0
MASON_EXIT_DIR=$d1 MASON_EXIT_SHARD=1/1 MASON_EXIT_SCRIPT_LIMIT_S=2 bash scripts/run-exit-scripts.sh >"$tmp/out" 2>"$tmp/err" || rc=$?
[ "$rc" -ne 0 ] || die "runner exited 0 with failing scripts"
grep -q "FAILED: .*s11.sh (over 2 s)" "$tmp/err" || die "over-limit script not reported"
grep -q "FAILED: .*s12.sh$" "$tmp/err" || die "failing script not reported"
grep -q "FAILED: .*s13.sh" "$tmp/err" && die "passing script reported as failed"
grep -qE '^s13: [0-9]+ s$' "$tmp/out" || die "elapsed seconds not printed"

# 2. weights: five scripts, two shards
d2=$tmp/two; mkdir "$d2"
for n in 11 12 13 14 15; do printf '#!/usr/bin/env bash\nexit 0\n' > "$d2/s$n.sh"; done
printf 's11 100\ns12 60\ns13 50\ns14 40\n' > "$d2/weights.txt"   # s15 unknown: default 10
plan() { MASON_EXIT_DIR=$d2 MASON_EXIT_DEFAULT_WEIGHT=10 MASON_EXIT_SHARD=$1 MASON_EXIT_PLAN=1 bash scripts/run-exit-scripts.sh | xargs -n1 basename | tr '\n' ' '; }
# greedy: s11->A(100), s12->B(60), s13->B(110), s14->A(140), s15->B(120)
[ "$(plan 1/2)" = "s11.sh s14.sh " ] || die "shard 1/2 got: $(plan 1/2)"
[ "$(plan 2/2)" = "s12.sh s13.sh s15.sh " ] || die "shard 2/2 got: $(plan 2/2)"
printf '#!/usr/bin/env bash\nexit 0\n' > "$d2/s16.sh"   # a new light script moves none of the others
[ "$(plan 1/2)" = "s11.sh s14.sh " ] || die "adding a script moved shard 1"

# 3. repetition scripts run their cargo test once at MASON_EXIT_REPEAT=1
mkdir "$tmp/bin"
cat > "$tmp/bin/cargo" <<SHIM
#!/usr/bin/env bash
echo x >> "$tmp/calls"
echo "test result: ok. 1 passed; 0 failed"
SHIM
chmod +x "$tmp/bin/cargo"
expect() { # script calls
  : > "$tmp/calls"
  PATH="$tmp/bin:$PATH" MASON_EXIT_REPEAT=1 bash "scripts/exit/$1.sh" >/dev/null 2>&1 || die "$1 failed under the shim"
  got=$(wc -l < "$tmp/calls" | tr -d ' ')
  [ "$got" = "$2" ] || die "$1 ran cargo $got times, expected $2"
}
expect s80 1; expect s99 1; expect s169 2; expect s170 4; expect s482 1; expect s497 1

# 4. the workflow
job=$(awk '/^  exit-scripts:/{f=1;next} f&&/^  [a-z]/{exit} f' .github/workflows/ci.yml)
echo "$job" | grep -q MASON_EXIT_REPEAT || die "ci.yml exit-scripts lacks MASON_EXIT_REPEAT"
echo "$job" | grep -q MASON_EXIT_SCRIPT_LIMIT_S || die "ci.yml exit-scripts lacks MASON_EXIT_SCRIPT_LIMIT_S"
echo "s668 OK"
