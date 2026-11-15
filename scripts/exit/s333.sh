#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s333 exit test: sidebar sections list tasks that are not waiting first.
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test -q -p mason-daemon --lib -- tasks_that_are_not_waiting_come_first_then_newest_first
echo "s333 OK"
