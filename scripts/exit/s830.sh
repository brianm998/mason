#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s830 exit test (#873): a narrowed remote check is sharded (exit-scripts sharding is s843's), a check that ended without a result
# is retried inside MASON, and a skip ends a check that waits.
#
# Not here: a scripted agent against a real masond and a fake GitHub over HTTP. The fake GitHub
# these tests use is the in-process one of crates/daemon/src/remote.rs, which drives the same
# `run_retrying` and `execute_sharded` that `run_check` calls.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

# The fake-GitHub tests: retry, skip cancel, a narrowed run as shards; the narrowed shard command.
cargo test -q -p mason-daemon --lib -- remote::tests::a_run_nobody_picks_up remote::tests::a_refused_push remote::tests::a_run_cancelled_at_the_limit remote::tests::two_misses remote::tests::a_red_run_is_a_result remote::tests::a_skip_while_a_run_waits remote::tests::a_narrowed_run_of_three_targets narrowed_shard_tests
cargo test -q -p mason-daemon --test it fallback_lease
echo "s830 ok"
