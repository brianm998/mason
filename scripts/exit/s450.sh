#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s450 exit test: every exit script that runs swift or gradle tests its skip variable, and
# s433, s435 and s438-s440 skip and exit 0 when both variables are set.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
bash scripts/check-exit-guards.sh
for n in 433 435 438 439 440; do
  MASON_SKIP_SWIFT=1 MASON_SKIP_COMPOSE=1 bash "scripts/exit/s$n.sh" >/dev/null
done
out=$(MASON_SKIP_SWIFT=1 MASON_SKIP_COMPOSE=1 bash scripts/exit/s438.sh)
echo "$out" | grep -q skipped || { echo "s450: s438 did not say it skipped" >&2; exit 1; }
echo "s450 OK"
