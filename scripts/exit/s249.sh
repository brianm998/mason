#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s249 exit test: scripts/run-changed-exit-scripts.sh runs only the exit scripts a change adds or
# edits relative to its merge base, prints one `PASS sNNN` / `FAIL sNNN` line per script, skips the
# real and manual ones, and exits non-zero when one fails. A temporary repository stands in.
set -euo pipefail
cd "$(dirname "$0")/../.."
root=$PWD
helper=$root/scripts/run-changed-exit-scripts.sh
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
cd "$tmp"
git init -q -b main repo
cd repo
g() { git -c user.name=t -c user.email=t@t "$@"; }
mkdir -p scripts/exit
printf '#!/bin/bash\nexit 0\n' > scripts/exit/s003.sh
printf '#!/bin/bash\nexit 1\n' > scripts/exit/s009.sh   # red, but this change does not touch it
echo base > README
g add -A && g commit -qm base
g checkout -qb change
printf '#!/bin/bash\necho ran-s001 >> "$MARKER"\nexit 0\n' > scripts/exit/s001.sh
printf '#!/bin/bash\necho boom\nexit 1\n' > scripts/exit/s002.sh
printf '#!/bin/bash\necho ran-real >> "$MARKER"\nexit 0\n' > scripts/exit/s004-real.sh
g add -A && g commit -qm change
export MARKER=$tmp/ran

echo "== one passes, one fails, the real one is skipped, the untouched red one is not run"
set +e
out=$(MASON_BASE=main bash "$helper" 2>&1); rc=$?
set -e
[ "$rc" -ne 0 ] || { echo "$out" >&2; echo "FAILED: a failing script did not fail the run" >&2; exit 1; }
for want in "PASS s001" "FAIL s002" "    | boom"; do
    echo "$out" | grep -qxF -- "$want" || { echo "$out" >&2; echo "FAILED: output lacks: $want" >&2; exit 1; }
done
echo "$out" | grep -q "s003\|s009\|s004" && { echo "$out" >&2; echo "FAILED: ran a script it should not" >&2; exit 1; }
[ "$(cat "$MARKER")" = "ran-s001" ] || { echo "FAILED: the real script ran" >&2; exit 1; }
# One result line per script, the shape the suite's regex reads.
[ "$(echo "$out" | grep -cE '^(PASS|FAIL) ')" = 2 ] || { echo "$out" >&2; echo "FAILED: not one line per script" >&2; exit 1; }

echo "== a change with no exit script passes with PASS none"
g checkout -q main
g checkout -qb docs
echo more >> README
g commit -qam docs
out=$(MASON_BASE=main bash "$helper" 2>&1) || { echo "$out" >&2; echo "FAILED: no scripts should pass" >&2; exit 1; }
[ "$out" = "PASS none" ] || { echo "$out" >&2; echo "FAILED: wanted PASS none" >&2; exit 1; }

echo "== the suite and its impact line are in the definition"
cd "$root"
grep -q '^\[suite.exit-scripts\]' .mason/mason.toml
grep -q '"exit-scripts"\]' .mason/mason.toml
grep -q '^"scripts/exit/\*\*" = \["cargo-workspace", "exit-scripts"\]' .mason/mason.toml
echo "s249 OK"
