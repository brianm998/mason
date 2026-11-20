#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s689 exit test: a suite with narrow_* keys, whole_later = "batch" and a batch tier listing it.
# A change touching a file no test names: the test step's note says the suite was not run, the
# suite ran nothing, and the batch tier's plan lists it (whole).
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test -q -p mason-daemon --test it a_suite_that_runs_whole_at_the_batch
cargo test -q -p mason-testing narrow
cargo test -q -p mason-landing whole_later
