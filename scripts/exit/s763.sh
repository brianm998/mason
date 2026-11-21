#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s763 exit test: s592 asserts the weighted greedy shard selection, not the old modulo formula, and passes.
set -uo pipefail
cd "$(dirname "$0")/../.."
fail=0
if grep -qF '(k - 1) % sn + 1' scripts/exit/s592.sh; then echo "FAIL s763: s592 names the modulo formula"; fail=1; fi
grep -qF 'owner[i] == si' scripts/exit/s592.sh || { echo "FAIL s763: s592 lost the shard check"; fail=1; }
out=$(bash scripts/exit/s592.sh 2>&1); rc=$?
[ "$rc" = 0 ] && printf '%s\n' "$out" | grep -q 'PASS s592' || { echo "FAIL s763: s592 does not pass: $out"; fail=1; }
if [ "$fail" = 0 ]; then echo "PASS s763"; else echo "FAIL s763"; fi
exit $fail
