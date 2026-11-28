#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1245 exit test: criteria name a check; develop reports and review grades each one, and a
# disagreement on a met criterion sends the task back naming it.
set -euo pipefail
cd "$(dirname "$0")/../.."
grep -q 'fn apply_review' crates/daemon/src/criteria.rs
grep -q 'criteria::apply_review' crates/daemon/src/jobs.rs
cargo test -q -p mason-daemon --lib scripted_explore_develop_review
cargo test -q -p mason-adapter-api plan_criteria_name_a_check_each
echo "s1245: ok"
