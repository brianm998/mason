#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s81 exit test: every git and shell call has a deadline. Unit tests for the process
# runner and git::run, the CPU reading's age, and a real masond against a remote that
# is a local TCP listener that never answers.
set -euo pipefail
cd "$(dirname "$0")/../.."

echo "== a hung git is killed at its deadline, with its helpers"
cargo test -q -p mason-landing -- deadline_tests proc::tests
echo "== a CPU reading older than 30 seconds is unknown"
cargo test -q -p mason-daemon --lib cpu::tests::an_old_reading_is_unknown
echo "== a stalled fetch does not block another task's cleanup"
cargo test -q -p mason-daemon --test fetch_deadline
echo "== real masond: port opens at once, the fetch stops at its deadline, cancel is not held up"
cargo test -q -p mason-daemon --test fetch_stall

echo "s81 OK"
