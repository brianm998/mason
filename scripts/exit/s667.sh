#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s667 exit test: a scripted base-branch run history where a shard fails once and then times
# out three times (GitHub's `cancelled` inside a failed run, with the timeout annotation)
# shows the timeout count, files exactly one "exceeds its time limit" task, ignores a
# superseded run, and a later green clears the line. No network.
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test -q -p mason-daemon --test it ci_watch::a_shard_that_fails_once_then_times_out
cargo test -q -p mason-daemon --lib ci_watch::tests
cargo test -q -p mason-core ci_watch
echo "s667: ok"
