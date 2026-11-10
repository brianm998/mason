#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s115 exit test: a real daemon with the scripted agent, an injected disk reading below the floor
# and `[stall] after = "2s"` (the test uses 1s): the alert list shows `stalled` naming the disk;
# raising the reading lets the task land and clears it. (The integration tests start the real
# daemon, real git, real step hosts and the scripted agent; the disk reading is injected.)
set -euo pipefail
cd "$(dirname "$0")/../.."

run() {
    local what=$1; shift
    echo "== $what"
    out=$(cargo test -q "$@" 2>&1) || { echo "$out" >&2; echo "FAILED: $what" >&2; exit 1; }
    echo "$out" | grep -E '^test result' | tail -1
}

run "a quiet pipeline raises stalled naming the disk, and clears when it moves" -p mason-daemon --test stall
run "the pure decision" -p mason-daemon --lib attention
run "stalled is routed to every channel" -p mason-core notify
run "stall.after is read" -p mason-daemon --lib notify
echo "s115 OK"
