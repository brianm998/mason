#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1203 exit test: health sessions are stamped on the tick clock (decision 1193).
set -euo pipefail
cd "$(dirname "$0")/../.."

grep -q '^Summary: ' docs/stages/s1203.md
cargo test -q -p mason-daemon --test it -- a_stuck_task_starts_one_session_and_a_second_tick_starts_none a_needs_owner_session_does_not_block_a_new_finding_and_times_out
