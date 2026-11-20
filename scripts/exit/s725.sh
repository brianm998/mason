#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s725 exit test: a fixture CI-watch state with landed and open tickets (the 2026-11-19 shape:
# five exit-script shards whose ticket landed, one job whose ticket is open) goes through the
# home view's banner builder: exactly one `ci_watch` banner, naming the open ticket, and one
# quiet "fix landed, not yet confirmed by a finished run" line; a green clears both; a job
# absent from agreement + 2 finished runs is forgotten.
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test -q -p mason-daemon --lib views::tests::the_home_ci_watch_banners_of_the_2026_11_19_shape
cargo test -q -p mason-core ci_watch::tests::held_lines_split_open_and_landed_and_group_shards
cargo test -q -p mason-core ci_watch::tests::a_job_absent_from_agreement_plus_two_finished_runs
cargo test -q -p mason-core ci_watch
echo "s725: ok"
