#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1180 exit test: the monitor-compose red of run 45883735030 is recorded with how to classify it.
set -euo pipefail
cd "$(dirname "$0")/../.."
d=docs/decisions/1170-monitor-compose-ubuntu-red-run-45883735030.md
[ -f "$d" ] || { echo "s1180: decision missing" >&2; exit 1; }
for want in 45883735030 0f7483b0c 'FAILED testcase' 'timed out waiting for task' 'Gradle or setup-gradle error' 1087 s1098; do
  grep -qF "$want" "$d" || { echo "s1180: decision does not mention: $want" >&2; exit 1; }
done
grep -q '^Summary: ' docs/stages/s1180.md || { echo "s1180: stage note has no Summary line" >&2; exit 1; }
bash -n scripts/exit/s13.sh
echo "s1180 passed"
