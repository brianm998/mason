#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s805 exit test: CI-watch auto-approval files confirmed reds approved, folds shards, holds the cap.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
cargo test -q -p mason-core --lib -- ci_watch::tests::auto_approve ci_watch::tests::a_long_red ci_watch::tests::a_red_an_open_task ci_watch::tests::shards_of_one_job ci_watch::tests::one_failure_on_two ci_watch::tests::the_cap_on_auto
cargo test -q -p mason-definition --lib -- watch_ci_auto_approve
echo "s805 passed"
