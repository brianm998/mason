#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s592 exit test: run-exit-scripts.sh ends a red run with a `FAILED scripts:` summary and still exits $fail.
set -uo pipefail
cd "$(dirname "$0")/../.."
f=scripts/run-exit-scripts.sh
fail=0
grep -q 'FAILED scripts:' "$f" || { echo "FAIL s592: no summary line"; fail=1; }
[ "$(tail -n 1 "$f")" = 'exit $fail' ] || { echo "FAIL s592: exit \$fail is not last"; fail=1; }
grep -q '(k - 1) % sn + 1' "$f" || { echo "FAIL s592: shard selection changed"; fail=1; }
if [ "$fail" = 0 ]; then echo "PASS s592"; else echo "FAIL s592"; fi
exit $fail
