#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1187 exit test: max_rounds counts red rounds only; s69's cases still hold.
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test -q -p mason-daemon --lib -- green_runs_do_not_spend reds_add_up conflict_runs_do_not_count an_approval_step_counts_only
bash scripts/exit/s69.sh
echo "s1187 OK"
