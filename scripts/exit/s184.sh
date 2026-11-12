#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# A takeover waits for running landing batches (s184): the drain tests hold a batch in its
# verify phase, assert the update waits and holds new batches, and that it ends when the batch does.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
cargo test -q -p mason-daemon --test takeover_drain
cargo test -q -p mason-daemon --lib a_requeued_task_counts_its_window_from_its_first_submission
cargo test -q -p mason-store --test store the_first_landing_submission_of_a_run_is_remembered
