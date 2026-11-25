#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1098 exit test: the monitor-compose red of run 45581279465 is recorded with how to classify it.
set -euo pipefail
cd "$(dirname "$0")/../.."
d=docs/decisions/1087-monitor-compose-ubuntu-red-run-45581279465.md
[ -f "$d" ] || { echo "s1098: decision missing" >&2; exit 1; }
for want in 45581279465 ff4fae6a6 'FAILED testcase' 'timed out waiting for task' 'Gradle or setup-gradle error' 1057 s1068; do
  grep -qF "$want" "$d" || { echo "s1098: decision does not mention: $want" >&2; exit 1; }
done
grep -q '^Summary: ' docs/stages/s1098.md || { echo "s1098: stage note has no Summary line" >&2; exit 1; }
bash -n scripts/exit/s13.sh
echo "s1098 passed"
