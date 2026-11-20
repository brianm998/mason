#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s698 exit test: a fixture history (no network) where `lint` and `test` are both red and a
# ticket citing the run names only `CI/lint` files `test` once; two shards red on the same
# script (s638) are filed once together; a failure an open task names is attached; a repeat
# of a landed task's failure is filed with that task named.
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test -q -p mason-core ci_watch::tests::a_ticket_citing_the_run_covers_only_the_job_it_names
cargo test -q -p mason-core ci_watch::tests::two_shards_with_the_same_failing_script
cargo test -q -p mason-core ci_watch::tests::a_failure_named_by_an_open_task
cargo test -q -p mason-core ci_watch::tests::a_failure_a_landed_task_named
cargo test -q -p mason-core ci_watch::tests::signatures_are_read
cargo test -q -p mason-core ci_watch
echo "s698: ok"
