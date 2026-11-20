#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s679 exit test: the monitor-compose red is recorded as undiagnosable from the log tail,
# with a stage note summary and a decision, and no code changed.
set -euo pipefail
cd "$(dirname "$0")/../.."

grep -q '^Summary: ' docs/stages/s679.md || { echo "s679: stage note has no Summary line" >&2; exit 1; }
d=docs/decisions/0672-monitor-compose-ubuntu-red-run-44615387131.md
[ -f "$d" ] || { echo "s679: decision missing" >&2; exit 1; }
grep -q '0620' "$d" && grep -q '0649' "$d" && grep -q '0630' "$d" && grep -q 's627' "$d" || { echo "s679: decision must cite 0620 and s627" >&2; exit 1; }
grep -q 'MASON_NO_GUI=1 bash scripts/exit/s13.sh' .github/workflows/ci.yml \
  || { echo "s679: ci.yml no longer runs s13.sh" >&2; exit 1; }
echo "s679: ok"
