#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s262 exit test: a develop check ranks behind a task already in test.
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test -q -p mason-daemon --lib -- a_develop_check_ranks_behind_a_task_already_in_test
echo "s262 OK"
