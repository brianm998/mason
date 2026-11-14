#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s260 exit test: waiting jobs share one scheduling pass and every one is still granted.
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test -q -p mason-daemon --lib -- a_waiters_pass_is_due_only_when_none_ran_lately waiting_jobs_share_one_pass_and_all_get_their_turn
echo "s260 OK"
