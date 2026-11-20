#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s718 exit test: s668 passes under a CI job's shard environment, whatever the shard.
set -euo pipefail
cd "$(dirname "$0")/../.."
grep -q '^unset MASON_EXIT_SHARD' scripts/exit/s668.sh || { echo "s718 FAILED: s668 does not clear MASON_EXIT_SHARD" >&2; exit 1; }
for shard in 5/6 1/6 6/6; do
  out=$(MASON_EXIT_SHARD=$shard MASON_EXIT_REPEAT=2 MASON_EXIT_SCRIPT_LIMIT_S=900 MASON_TESTS_ALREADY_RAN=1 \
    MASON_SKIP_SWIFT=1 MASON_SKIP_COMPOSE=1 bash scripts/exit/s668.sh) || { echo "s718 FAILED: s668 failed on shard $shard" >&2; exit 1; }
  echo "$out" | grep -q '^s668 OK$' || { echo "s718 FAILED: no s668 OK on shard $shard" >&2; exit 1; }
done
echo "s718 OK"
