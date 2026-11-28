#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Stage 1315 exit test: a test daemon does not inherit the step the test suite runs in.
# The two exit-script tests of suite_builds pass with a pipeline test step's variables set.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
if [ -n "${MASON_TESTS_ALREADY_RAN:-}" ]; then
  echo "s1315 OK (the cargo suite ran these tests)"
  exit 0
fi
cargo build -q --workspace --bins
echo "== the exit-script tests, run as inside a test step"
if ! out=$(env MASON_STEP=test MASON_TASK=1 MASON_TESTS_ALREADY_RAN=1 MASON_EXIT_IN_CHECK=1 \
  cargo test -q -p mason-daemon --test it suite_builds::exit_scripts_ 2>&1) \
  || ! grep -q 'test result: ok. 2 passed' <<<"$out"; then
  tail -40 <<<"$out" >&2
  exit 1
fi
echo "s1315 OK"
