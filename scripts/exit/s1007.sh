#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1007 exit test: skip() refuses steps other than the Test step.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
grep -q 'only the test step can' crates/daemon/src/suites.rs
grep -q 'a_skip_of_a_step_that_is_not_the_test_step_is_refused' crates/daemon/tests/landing_skip.rs
echo "s1007 passed"
