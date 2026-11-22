#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test -p mason-daemon --test it -- \
  landing::a_red_lint_run_here_parks_the_task_with_file_and_line \
  landing::a_lint_tier_set_to_github_goes_there_and_holds_no_local_compile_slot_while_queued \
  landing::the_lint_tier_runs_here_while_the_switch_says_github
