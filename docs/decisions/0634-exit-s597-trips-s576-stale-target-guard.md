<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
# 0634: rewrite s597's pattern instead of excluding it from s576

Status: accepted (s641).

Context: `CI/exit-scripts (ubuntu-latest, 2)` was red on develop. s576's guard greps scripts/exit for `--test (limits|stop)\b` and found s597.sh:10, which contains that text in a negative check, so s576 failed with "stale test target".

Decision: s597's pattern becomes `--test \(apns\|limits\)`, which checks the same strings without the guarded substring. s576 and its exclude list are unchanged, so the guard still covers every other script.

Consequence: any future script that names those targets literally will trip the guard and must build the pattern the same way.
