#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# A GitHub run starts by dispatch with its shard list and runner as inputs, no plan job (s856, decision 0849).
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
cargo test -q -p mason-daemon execute_pushes_the_ref_then_starts_the_run_once
cargo test -q -p mason-daemon a_restarted_daemon_finds_the_run_by_its_sha
cargo test -q -p mason-daemon the_run_workflow_is_one_wave
cargo test -q -p mason-adapters the_dispatch_trigger_is_read_in_each_form
echo "s856 ok"
