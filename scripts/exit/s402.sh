#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s402 exit test: every wait cause has a non-empty class.
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test -q -p mason-scheduler --lib -- every_wait_cause_has_a_class
cargo test -q -p mason-scheduler --lib -- a_cpu_wait_and_a_test_suite_wait_are_classed_by_their_cause
echo "s402 OK"
