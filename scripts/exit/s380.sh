#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s380 exit test: a suite already green for the tree takes no lease when its step runs again.
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test -q -p mason-daemon --test scheduling -- a_suite_already_green_for_the_tree_takes_no_lease_when_its_step_runs_again
echo "s380 OK"
