#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Stage 576 exit test: exit scripts name the `it` test binary, not the removed limits/stop targets.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
if grep -rnE --exclude=s576.sh -- '--test (limits|stop)\b' scripts/exit; then
  echo "s576 FAILED: stale test target" >&2; exit 1
fi
cargo test -q -p mason-daemon --test it limits::the_views_carry_the_syspolicyd_reading_and_limit
cargo test -q -p mason-daemon --test it stop::a_running_step_reads_limits_changed_by_a_registration_reload
cargo test -q -p mason-daemon --test it limits::a_busy_syspolicyd_holds_further_compiles_but_never_the_first
echo "s576 OK"
