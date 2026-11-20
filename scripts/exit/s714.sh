#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s714 exit test: the monitor-compose red is recorded as undiagnosable from the log tail,
# with a stage note summary and a decision, and no code changed.
set -euo pipefail
cd "$(dirname "$0")/../.."

grep -q '^Summary: ' docs/stages/s714.md || { echo "s714: stage note has no Summary line" >&2; exit 1; }
d=docs/decisions/0707-monitor-compose-ubuntu-red-run-44767972773.md
[ -f "$d" ] || { echo "s714: decision missing" >&2; exit 1; }
grep -q '0690' "$d" && grep -q '0672' "$d" && grep -q 's697' "$d" || { echo "s714: decision must cite 0690, 0672 and s697" >&2; exit 1; }
grep -q 'MASON_NO_GUI=1 bash scripts/exit/s13.sh' .github/workflows/ci.yml \
  || { echo "s714: ci.yml no longer runs s13.sh" >&2; exit 1; }
grep -q 'timeout-minutes: 30' .github/workflows/ci.yml \
  || { echo "s714: ci.yml timeout-minutes: 30 gone" >&2; exit 1; }
echo "s714: ok"
