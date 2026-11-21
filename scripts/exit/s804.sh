#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s804 exit test: a batch narrows to its union, runs whole for Cargo.lock, needs a full-test
# schedule, and goes back to the whole suite when the last full test is red or too old.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
cargo test -q -p mason-core --lib -- full_test::tests::a_fresh_green full_test::tests::a_red_is_in_force full_test::tests::an_old_green
cargo test -q -p mason-landing --lib -- narrow_reads_only
cargo test -q -p mason-daemon --lib -- narrow_batch_tests
echo "s804 passed"
