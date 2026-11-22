#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s901 exit test: static checks that s09 reads only the steps array and s748/s750 capture hold output.
set -uo pipefail
cd "$(dirname "$0")/../.."
fail=0
grep -q '\["steps"\]' scripts/exit/s09.sh || { echo "FAIL s901: steps_of does not read the steps array"; fail=1; }
grep -q "grep '\"name\":'" scripts/exit/s09.sh && { echo "FAIL s901: steps_of still greps every name"; fail=1; }
for s in s748 s750; do
  grep -q 'hold.err' scripts/exit/$s.sh && grep -q 'hold.rc' scripts/exit/$s.sh || { echo "FAIL s901: $s does not capture hold output"; fail=1; }
  grep -q 'kill -0 "\$holder" || fail "the held slots were released"' scripts/exit/$s.sh || { echo "FAIL s901: $s lost its check"; fail=1; }
done
grep -qF 'owner[i] == si' scripts/exit/s592.sh && grep -qF 'tot[best] += wt[i]' scripts/run-exit-scripts.sh || { echo "FAIL s901: shard strings"; fail=1; }
[ "$fail" = 0 ] && echo "PASS s901"
exit $fail
