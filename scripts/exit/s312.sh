#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s312 exit test: MASON's own checkouts run the definition's worktree setup.
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test -q -p mason-definition --lib -- worktree_setup_is_the_command_steps_before_the_first_agent_change
cargo test -q -p mason-daemon --test it -- paying_a_debt_runs_the_worktree_setup_first
echo "s312 OK"
