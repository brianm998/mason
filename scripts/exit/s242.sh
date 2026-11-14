#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s242 exit test: 30 waiters share one pass per tick, and readers and release
# complete while a lock-file probe is blocked.
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test -q -p mason-daemon --lib -- one_pass_serves_every_waiter_per_tick readers_and_release_do_not_wait_for_a_blocked_lock_file_probe
echo "s242: ok"
