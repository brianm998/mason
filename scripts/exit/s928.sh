#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# CI-watch jobs are keyed by the workflow's name, and old title-keyed state is dropped (s928, 0921).
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
cargo test -q -p mason-adapters --test github a_base_run_is_named_by_its_workflow
cargo test -q -p mason-core red_runs_of_one_job_on_different_commits
cargo test -q -p mason-daemon --test it saved_state_keyed_by_a_commit_message
echo "s928: all checks passed"
