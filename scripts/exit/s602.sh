#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s602 exit test: run-exit-scripts.sh names every failing script in a final line.
# It runs the real runner in a scratch repo with fake scripts (the real shard would recurse into this one).
set -euo pipefail
here=$(cd "$(dirname "$0")/../.." && pwd)
t=$(mktemp -d); trap 'rm -rf "$t"' EXIT
cd "$t"; git init -q .; mkdir -p scripts/exit
cp "$here/scripts/run-exit-scripts.sh" scripts/
printf 'exit 0\n' > scripts/exit/s01.sh
printf 'exit 1\n' > scripts/exit/s02.sh
printf 'exit 1\n' > scripts/exit/s03.sh
fail() { echo "s602: $1" >&2; exit 1; }
rc=0; out=$(bash scripts/run-exit-scripts.sh 2>&1) || rc=$?
[ "$rc" -eq 1 ] || fail "expected exit 1, got $rc"
echo "$out" | tail -1 | grep -qx 'FAILED scripts: scripts/exit/s02.sh scripts/exit/s03.sh' || fail "last line wrong: $(echo "$out" | tail -1)"
rm scripts/exit/s02.sh scripts/exit/s03.sh
out=$(bash scripts/run-exit-scripts.sh 2>&1) || fail "green tree exited nonzero"
! echo "$out" | grep -q 'FAILED' || fail "green run printed FAILED"
bash "$here/scripts/exit/s585.sh" | grep -qx 's585: ok' || fail "s585 prints no verdict"
echo "s602: ok"
