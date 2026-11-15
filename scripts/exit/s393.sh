#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s393 exit test: a skipped suite's debt closes by itself when a later task's run is green.
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test -q -p mason-daemon --test testing -- a_debt_closes_by_itself_when_the_suite_later_goes_green_on_a_base_with_the_task
cargo test -q -p mason-daemon --test testing -- a_debt_never_closes_on_a_green_at_a_base_without_the_tasks_commit
cargo test -q -p mason-daemon --test testing -- a_debt_whose_suite_is_still_red_stays_open_and_is_run_once_when_idle
echo "s393 OK"
