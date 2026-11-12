#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s169 exit test: the two formerly flaky update tests each pass 10 runs in a row.
set -uo pipefail
cd "$(dirname "$0")/../.."
n=${S169_RUNS:-10}
fail=0
for name in \
  a_build_that_crashes_is_rolled_back_by_itself_with_an_alert \
  an_update_while_an_agent_session_runs_does_not_interrupt_it; do
  for i in $(seq 1 "$n"); do
    if cargo test -q -p mason-daemon --test update "$name" -- --exact >/dev/null 2>&1; then
      echo "$name run $i: green"
    else
      echo "$name run $i: RED"
      fail=1
    fi
  done
done
[ "$fail" -eq 0 ]
