#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Rewrites scripts/exit/weights.txt from CI shard logs: reads files (or stdin) holding the
# runner's `sNN: N s` lines and keeps scripts that took 120 s or more, the heaviest first.
# Usage: gh run view <id> --log | bash scripts/refresh-exit-weights.sh
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
out=${MASON_EXIT_WEIGHTS:-scripts/exit/weights.txt}
cat "$@" | grep -oE 's[0-9]+: [0-9]+ s$' \
  | awk '{ sub(":", "", $1); if (!($1 in m) || $2 > m[$1]) m[$1] = $2 }
         END { for (k in m) if (m[k] >= 120) print k, m[k] }' \
  | sort -k2,2nr -k1,1 > "$out.new"
mv "$out.new" "$out"
echo "wrote $out"
