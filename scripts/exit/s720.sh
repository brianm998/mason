#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s720 exit test: s640 does not re-run s426, s664 builds once, and every exit script is in one shard (#709).
set -euo pipefail
cd "$(dirname "$0")/../.."
! grep -Eq '^ *bash scripts/exit/s426\.sh' scripts/exit/s640.sh || { echo "s640 re-runs s426" >&2; exit 1; }
grep -q -- '--no-run' scripts/exit/s664.sh
! grep -E '^ *out=.*cargo test' scripts/exit/s664.sh || { echo "s664 calls cargo in its loop" >&2; exit 1; }
grep -q 'MASON_EXIT_SHARD' .github/workflows/ci.yml
grep -q 'MASON_EXIT_REPEAT' .github/workflows/ci.yml
plan=$(for i in 1 2 3 4 5 6; do
    MASON_EXIT_SHARD=$i/6 MASON_EXIT_PLAN=1 bash scripts/run-exit-scripts.sh
done | grep -oE 's[0-9]+\.sh' | sort)
want=$(ls scripts/exit | grep -E '^s[0-9]+\.sh$' | sort)
[ "$plan" = "$want" ] || { echo "shards do not cover each script once" >&2; diff <(echo "$plan") <(echo "$want") >&2 || true; exit 1; }
echo "s720 OK"
