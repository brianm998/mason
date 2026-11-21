#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s739 exit test: the landing_yield test passes at the base.
set -uo pipefail
cd "$(dirname "$0")/../.."
cargo test -q -p mason-daemon --test it a_draining_update_stops_a_red_batch_between_runs_and_the_next_batch_reuses_its_results
