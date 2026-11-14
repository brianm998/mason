#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s243 exit test: with a command blocked and view-pool threads held, a session
# still answers pings and delivers the TASK_DETAIL view.
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test -q -p mason-daemon --test protocol -- a_blocked_command a_burst_of_ticks
