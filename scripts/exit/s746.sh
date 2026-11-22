#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s746 exit test: the tests that were red only on GitHub's ubuntu-latest pass by name.
set -euo pipefail
cd "$(dirname "$0")/../.."

grep -q '^Summary: ' docs/stages/s746.md || { echo "s746: stage note has no Summary line" >&2; exit 1; }
[ -f docs/decisions/0739-linux-green-cargo-workspace.md ] || { echo "s746: decision missing" >&2; exit 1; }
cargo test -q -p mason-daemon --lib -- iso_times_and_ref_names a_skip_while_queued_or_running a_second_task_changing_only_crate_b
cargo test -q -p mason-daemon --test it -- a_pr_wait_shows_the_hosts_checks a_draining_update_stops_a_red_batch a_hot_die_holds_further_machine_work
cargo test -q -p mason-pipeline --bin mason -- no_tracked_file_but_the_definition_is_unmapped a_swiftpm_project_gets_a_mac_runner_and_the_xcode_its_command_names
cargo test -q -p mason-pipeline --test remote_dispatch
echo "s746: ok"
