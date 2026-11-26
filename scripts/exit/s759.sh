#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s759 exit test: in a one-commit clone with no base, run-changed-exit-scripts.sh uses
# .mason/changed-files.txt instead of a merge base; with neither, it still fails as before.
# The snapshot test in crates/daemon/src/remote.rs covers the file's creation.
set -euo pipefail
# The CI shard env must not reach a one-commit repo that has no run-exit-scripts.sh, and the
# run's time budget would add a summary line to the output compared below.
unset MASON_EXIT_SHARD MASON_EXIT_PLAN MASON_RUN_BUDGET_S
cd "$(dirname "$0")/../.."
root=$PWD
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
cd "$tmp"
git init -q -b main repo
cd repo
g() { git -c user.name=t -c user.email=t@t "$@"; }
mkdir -p scripts/exit .mason
cp "$root/scripts/run-changed-exit-scripts.sh" "$root/scripts/select-exit-scripts.sh" scripts/
printf '#!/bin/bash\nexit 0\n' > scripts/exit/s001.sh
printf '#!/bin/bash\nexit 1\n' > scripts/exit/s002.sh
g add -A && g commit -qm one
echo "== no list, no base: fails as before"
set +e; out=$(MASON_BASE=nope bash scripts/run-changed-exit-scripts.sh 2>&1); rc=$?; set -e
[ "$rc" -ne 0 ] && echo "$out" | grep -qx "FAIL no-merge-base-with-nope" || { echo "$out" >&2; echo "FAILED" >&2; exit 1; }
echo "== list: the named script runs"
printf 'scripts/exit/s001.sh\n' > .mason/changed-files.txt
out=$(MASON_BASE=nope bash scripts/run-changed-exit-scripts.sh 2>&1) || { echo "$out" >&2; echo "FAILED" >&2; exit 1; }
[ "$out" = "PASS s001" ] || { echo "$out" >&2; echo "FAILED: wanted PASS s001" >&2; exit 1; }
printf 'scripts/exit/s002.sh\n' > .mason/changed-files.txt
set +e; out=$(bash scripts/run-changed-exit-scripts.sh 2>&1); rc=$?; set -e
[ "$rc" -ne 0 ] && echo "$out" | grep -qx "FAIL s002" || { echo "$out" >&2; echo "FAILED" >&2; exit 1; }
echo "s759 OK"
