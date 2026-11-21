#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s743 exit test: a develop agent's run_check is placed by the switch and the task's choice.
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test -p mason-daemon --test it -- a_develop_checks_suite_is_placed_by_the_switch_and_the_tasks_choice
