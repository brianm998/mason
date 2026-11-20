#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s697 exit test: the monitor-compose red is recorded as undiagnosable from the log tail,
# with a stage note summary and a decision, and no code changed.
set -euo pipefail
cd "$(dirname "$0")/../.."

grep -q '^Summary: ' docs/stages/s697.md || { echo "s697: stage note has no Summary line" >&2; exit 1; }
d=docs/decisions/0690-monitor-compose-ubuntu-red-run-44697853075.md
[ -f "$d" ] || { echo "s697: decision missing" >&2; exit 1; }
grep -q '0620' "$d" && grep -q '0672' "$d" && grep -q '0649' "$d" && grep -q '0630' "$d" && grep -q 's627' "$d" || { echo "s697: decision must cite 0620 and s627" >&2; exit 1; }
grep -q 'MASON_NO_GUI=1 bash scripts/exit/s13.sh' .github/workflows/ci.yml \
  || { echo "s697: ci.yml no longer runs s13.sh" >&2; exit 1; }
echo "s697: ok"
