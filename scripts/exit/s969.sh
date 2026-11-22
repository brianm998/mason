#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# An owner retry or Land now forgets the cached red landing runs of the task; an automatic
# re-submit keeps reusing them (s969, 0962).
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
cargo test -q -p mason-store --test store -- --exact \
  forgetting_a_tasks_red_runs_keeps_greens_and_other_tasks
cargo test -q -p mason-daemon --test it -- --exact \
  landing_forget_red::an_automatic_re_submit_reuses_the_red_result \
  landing_forget_red::an_owner_retry_from_land_runs_the_suite_again \
  landing_forget_red::land_now_on_a_task_with_a_cached_red_runs_the_suite_again
echo "s969: all checks passed"
