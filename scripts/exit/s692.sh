#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s692 exit test: a 4-task batch with one culprit runs the whole suite twice; the rerun and
# every bisect run only the failing test.
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test -p mason-daemon --test it -- a_red_batch_reruns_and_bisects_on_the_failing_tests
cargo test -p mason-daemon --lib -- alone_tests
echo "s692: ok"
