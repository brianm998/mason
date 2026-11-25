#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1100 exit test: s1026 and s1068 follow the signal-based Compose settle().
set -euo pipefail
cd "$(dirname "$0")/../.."
for s in s1026 s1068; do
  f=scripts/exit/$s.sh
  if grep -E 'QUIET_TICKS|views at' "$f"; then
    echo "s1100: $f still checks for the removed quiet-window text" >&2
    exit 1
  fi
  grep -qF 'Settled is a signal, not a quiet window' "$f"
  grep -qF 'applied up to ' "$f"
  bash "$f"
done
grep -q '^Summary: ' docs/stages/s1100.md
echo "s1100 passed"
