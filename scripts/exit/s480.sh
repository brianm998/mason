#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s480 exit test: the sccache spike's decision record has its measured rows and a recommendation.
set -euo pipefail
cd "$(dirname "$0")/../.."
f=docs/decisions/0473-sccache-workspace-crates.md
[ -f "$f" ]
rows=$(grep -cE '^\| (A|B|C|D) (cold|rebuild)' "$f")
[ "$rows" -ge 10 ] || { echo "s480: only $rows measured rows"; exit 1; }
grep -q '^## Recommendation' "$f"
grep -q '^## Not verified' "$f"
echo "s480: $rows measured rows, recommendation present"
