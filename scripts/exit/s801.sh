#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s801 exit test (#778): a test's recorded time leaves out its wait for a scratch world, and
# shard lists balance by run time. Runs the parser and shard tests.
set -euo pipefail
cd "$(dirname "$0")/../.."
grep -q 'mason-world-wait' crates/daemon/tests/support/mod.rs
cargo test -q -p mason-testing a_world_wait_line_is_subtracted
cargo test -q -p mason-daemon --lib shard_lists_from_times_with_and_without_a_wait_differ
echo "s801: ok"
