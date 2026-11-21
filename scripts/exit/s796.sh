#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s796 exit test: stage note present and the three landing tests still exist (grep only).
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
grep -q '^Summary: ' docs/stages/s796.md
for t in the_lint_tier_runs_here_while_the_switch_says_github \
  a_lint_tier_set_to_github_goes_there_and_holds_no_local_compile_slot_while_queued \
  a_red_lint_run_here_parks_the_task_with_file_and_line; do
  grep -q "fn $t" crates/daemon/tests/it/landing.rs
done
grep -q 'tier.place' crates/daemon/src/landing.rs
echo ok
