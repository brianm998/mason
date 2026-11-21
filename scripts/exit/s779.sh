#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s779 exit test: the daemon test wait floor is 900 s unless MASON_TEST_MIN_WAIT_S says otherwise;
# the helper tests pass with the variable unset and set.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
env -u MASON_TEST_MIN_WAIT_S cargo test -q -p mason-daemon --test holds wait_tests
MASON_TEST_MIN_WAIT_S=180 cargo test -q -p mason-daemon --test holds wait_tests
cargo test -q -p mason-daemon --lib remote_runs_sit
echo "s779: ok"
