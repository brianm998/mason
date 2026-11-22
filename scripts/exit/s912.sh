#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s912 exit test: monitor-compose red run 45005421555 is recorded as undiagnosable, with a
# stage note summary and a decision, and no code changed.
set -euo pipefail
cd "$(dirname "$0")/../.."

grep -q '^Summary: ' docs/stages/s912.md || { echo "s912: stage note has no Summary line" >&2; exit 1; }
d=docs/decisions/0905-monitor-compose-ubuntu-red-run-45005421555.md
grep -q '0690' "$d" && grep -q '0707' "$d" && grep -q 's714' "$d" || { echo "s912: decision must cite 0690, 0707 and s714" >&2; exit 1; }
grep -q '0690' "$d" && grep -q '0672' "$d" && grep -q 's697' "$d" || { echo "s912: decision must cite 0690, 0707 and s714" >&2; exit 1; }
grep -q 'MASON_NO_GUI=1 bash scripts/exit/s13.sh' .github/workflows/ci.yml \
  || { echo "s912: ci.yml no longer runs s13.sh" >&2; exit 1; }
grep -q 'timeout-minutes: 30' .github/workflows/ci.yml \
  || { echo "s912: ci.yml timeout-minutes: 30 gone" >&2; exit 1; }
echo "s912: ok"
