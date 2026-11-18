#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s577 exit test: the s553 exit test runs through the `it` harness and finds its test.
set -euo pipefail
cd "$(dirname "$0")/../.."
grep -q 'fn a_skip_with_no_step_name_ends_the_base_red_wait_and_records_the_debt' crates/daemon/tests/it/base_red.rs
grep -q -- '--test it a_skip_with_no_step_name' scripts/exit/s553.sh
out=$(cargo test -p mason-daemon --test it a_skip_with_no_step_name 2>&1) || { echo "$out" >&2; exit 1; }
echo "$out" | grep -q 'test result: ok. [1-9]' || { echo "$out" >&2; exit 1; }
echo "s577: ok"
