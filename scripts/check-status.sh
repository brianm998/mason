#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Fails when the status block in README.md is not what scripts/gen-status.sh builds from the
# stage notes. Fix with: bash scripts/gen-status.sh
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
begin='<!-- status:begin -->'
end='<!-- status:end -->'
have=$(awk -v b="$begin" -v e="$end" '$0 == b { on = 1 } on { print } $0 == e { on = 0 }' README.md)
want=$(bash scripts/gen-status.sh --print)
if [ "$have" != "$want" ]; then
  echo "README.md's status block is out of date; run: bash scripts/gen-status.sh" >&2
  diff <(echo "$have") <(echo "$want") >&2 || true
  exit 1
fi
echo "status block ok"
