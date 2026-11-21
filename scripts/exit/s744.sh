#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s744 exit test: the run snapshot holds the base's workflows and refuses one that runs on run refs.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
cargo test -q -p mason-daemon --lib the_snapshot_holds_the_dirty_worktree_and_the_trusted_overlay
cargo test -q -p mason-daemon --lib a_base_workflow_that_runs_on_run_refs_refuses_the_snapshot
