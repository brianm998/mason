#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s666 exit test: the old "test step runs the whole workspace" sentences are gone and the new tests pass.
set -euo pipefail
cd "$(dirname "$0")/../.."
for s in "The test step runs the whole workspace" "the full run comes before you submit" "it still runs everything"; do
    if grep -n "$s" crates/daemon/src/jobs.rs crates/adapter-api/src/contract.rs; then
        echo "FAILED: old sentence remains: $s" >&2
        exit 1
    fi
done
cargo test -q -p mason-adapter-api full_with_why_reaches_the_handler_and_why_is_optional
cargo test -q -p mason-daemon --test it -- a_scoped_run_check an_unscoped_run_check after_a_red_full_run
