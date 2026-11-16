#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s452 exit test: the seven exit scripts that were still red on CI (s09, s19, s40, s54, s130,
# s393, s396) all pass, run the way run-exit-scripts.sh runs them.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
if [ -z "${MASON_CPU_FILE:-}" ]; then
  cpu=$(mktemp); echo 0 > "$cpu"; export MASON_CPU_FILE="$cpu"
  trap 'rm -f "$cpu"' EXIT
fi
for s in s09 s19 s40 s54 s130 s393 s396; do
  echo "== scripts/exit/$s.sh"
  bash "scripts/exit/$s.sh" || { echo "FAILED: scripts/exit/$s.sh" >&2; exit 1; }
done
echo "s452 OK"
