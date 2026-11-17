#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s422 exit test: a run_check carries its suite's expected time to the broker, and the daemon is clippy clean.
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo clippy -q -p mason-daemon --all-targets -- -D warnings
cargo test -q -p mason-scheduler --test properties -- short_work_runs_while_the_machine_drains_and_long_or_unknown_work_waits nothing_passes_a_waiting_exclusive_request_when_a_holder_has_no_estimate
cargo test -q -p mason-daemon --test it suite_builds::
echo "s422 OK"
