#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s762 exit test: the stage note for the Linux exit-scripts red exists and has its Summary line.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
bash scripts/check-exit-guards.sh
grep -q '^Summary: ' docs/stages/s762.md
ls docs/decisions/0755-*.md >/dev/null
echo "s762 OK"
