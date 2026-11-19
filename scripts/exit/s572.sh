#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s572 exit test: the Compose core tests pass.
set -uo pipefail
if [ -n "${MASON_SKIP_COMPOSE:-}" ] || ! command -v gradle >/dev/null; then
  echo "s572 skipped: MASON_SKIP_COMPOSE is set (CI's monitor-compose job and s13 run :core:test)"; exit 0
fi
cd "$(dirname "$0")/../../clients/compose" && gradle :core:test --no-daemon -q
