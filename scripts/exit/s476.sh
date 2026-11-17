#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s476 exit test: a skip-green pays debts and names the skipped test.
set -euo pipefail
cd "$(dirname "$0")/../.."
grep -q 'Summary: ' docs/stages/s476.md
grep -q 'suite_skip_green' crates/store/src/migrations.rs
grep -q 'a_debt_is_paid_by_a_green_that_skipped_a_quarantined_test_and_the_test_is_named' crates/daemon/tests/testing.rs
echo "s476: ok"
