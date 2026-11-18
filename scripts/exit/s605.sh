#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Stage 605 exit test: no exit script puts grep's --exclude/--include after `--`, and s576's grep is quiet.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
if grep -nE -- 'grep .* -- .*--(exclude|include)' $(ls scripts/exit/*.sh | grep -v '/s605\.sh$'); then
  echo "s605 FAILED: grep option after the -- separator" >&2; exit 1
fi
err=$(mktemp); trap 'rm -f "$err"' EXIT
grep -rnE --exclude=s576.sh -- '--test (limits|stop)\b' scripts/exit >/dev/null 2>"$err" || true
if [ -s "$err" ]; then cat "$err" >&2; echo "s605 FAILED: grep wrote to stderr" >&2; exit 1; fi
echo "s605 OK"
