#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s385 exit test: short work runs while the machine drains for an exclusive request; nothing
# else passes it; suites record their times.
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test -q -p mason-scheduler --test properties -- short_work_runs_while_the_machine_drains nothing_passes_a_waiting_exclusive with_aging_every_request_is_eventually_granted
cargo test -q -p mason-store --test suite_times
cargo test -q -p mason-daemon --test scheduling -- a_suites_run_time_is_recorded_and_gives_an_estimate_after_two_runs
echo "s385 OK"
