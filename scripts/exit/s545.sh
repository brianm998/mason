#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s545 exit test: a daemon-test wait that can no longer succeed fails at once. The helper's own
# unit tests pass, and one deliberately failing scratch-daemon scenario (off unless
# MASON_EARLY_EXIT_DEMO=1) fails with the early-exit message, not "timed out after 900s".
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test -p mason-daemon --test early_exit_demo
cargo test -p mason-daemon --test early_exit_demo wait_tests
out=$(MASON_EARLY_EXIT_DEMO=1 cargo test -p mason-daemon --test early_exit_demo 2>&1) && {
  echo "s545: the demo scenario should fail" >&2; exit 1; }
echo "$out" | grep -q 'reached Canceled while waiting for Landed' || { echo "$out" >&2; echo "s545: not the early-exit message" >&2; exit 1; }
if echo "$out" | grep -q 'timed out after'; then echo "s545: it sat out the floor" >&2; exit 1; fi
echo "s545: ok"
