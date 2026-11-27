#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1231 exit test: a full-CI check holder gives no run-length bound to stuck:wait (decision 1221).
set -euo pipefail
cd "$(dirname "$0")/../.."

cargo test -q -p mason-core --lib check_holders_give_no_run_length_bound
cargo test -q -p mason-daemon --lib is_check_lease_matches_only_agent_check_labels
echo "OK s1231"
