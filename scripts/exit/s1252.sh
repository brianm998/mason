#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1252 exit test: a red run_check result names the failing tests and their panics from the
# whole log, and a scoped run that ran 0 tests is not green.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
grep -q 'pub fn failure_digest' crates/daemon/src/rerun.rs
grep -q 'crate::rerun::failure_digest' crates/daemon/src/jobs.rs
grep -q 'NOT GREEN' crates/daemon/src/jobs.rs
cargo test -p mason-daemon --lib rerun::tests:: -- --quiet
echo "s1252 passed"
