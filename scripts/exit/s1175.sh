#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Stage 1175 exit test: stuck:long, health session gating, health proposals past the self cap.
# The daemon-level flow (a held develop step with check rounds raises stuck:long:<id>) is the
# test `a_busy_task_is_stuck_long_and_the_prompt_names_the_restart`, which uses the fake adapter.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
cargo test -q -p mason-core a_busy_task_in_one_state_for_hours_is_stuck_long
cargo test -q -p mason-daemon --test it -- \
  a_busy_task_is_stuck_long_and_the_prompt_names_the_restart \
  a_needs_owner_session_does_not_block_a_new_finding_and_times_out \
  a_health_proposal_passes_the_weekly_self_task_cap_once
