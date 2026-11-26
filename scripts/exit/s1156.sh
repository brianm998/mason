#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1156 exit test: the cause line of a red run names the failing line, not a `== ` heading or
# prose that merely contains the word "error" (#1200). The runner runs over scratch scripts.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
d=$(mktemp -d /tmp/mason-s1156.XXXXXX)
trap 'rm -rf "$d"' EXIT
fail() { echo "FAILED: $*" >&2; printf '%s\n' "${out:-}" >&2; exit 1; }

# a heading and a title that both say "error", then the real failure
cat > "$d/s31.sh" <<'X'
echo '== 1. a retry loop on a permanent error, planted in recorded history'
echo '   #2 Retry loop on a permanent error: unknown model'
echo 'not ok 2 - the_real_check'
exit 1
X
# s22's shape: the failure is a wait that gave up
cat > "$d/s32.sh" <<'X'
echo '== 1. a retry loop on a permanent error'
echo "timed out waiting for the analyst's proposal" >&2
echo 'a later log line'
exit 1
X
# a heading that says FAIL is still a heading; a compiler diagnostic is a failure
cat > "$d/s33.sh" <<'X'
echo '== 2. what a FAILED run reports'
echo 'error[E0425]: cannot find value `x` in this scope'
exit 1
X
# nothing that reports a failure: the last line, not the heading
cat > "$d/s34.sh" <<'X'
echo '== 3. the error path'
echo 'the last thing said'
exit 1
X
echo 'echo "== an error that is only a heading"' > "$d/s35.sh"
printf 's31 1\ns32 1\ns33 1\ns34 1\ns35 1\n' > "$d/weights.txt"

out=$(MASON_EXIT_DIR="$d" MASON_EXIT_WEIGHTS="$d/weights.txt" MASON_EXIT_SHARD=1/1 \
  bash scripts/run-exit-scripts.sh 2>&1) && fail "the runner should have failed"

expect() {  # NAME CAUSE
  printf '%s\n' "$out" | grep -qxF "  cause $1: $2" || fail "cause of $1 is not '$2'"
}
expect s31 'not ok 2 - the_real_check'
expect s32 "timed out waiting for the analyst's proposal"
expect s33 'error[E0425]: cannot find value `x` in this scope'
expect s34 'last output: the last thing said'
! printf '%s\n' "$out" | grep -q 'cause s35' || fail "a passing script has a cause"

grep -q '^Summary: ' docs/stages/s1156.md
echo "s1156 passed"
