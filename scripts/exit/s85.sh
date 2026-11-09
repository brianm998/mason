#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s85 exit test: a real daemon with the scripted agent and an injected disk reading below the
# floor. A filed task waits and says why, and the alerts list the disk alert. Raising the reading
# lets the task run and land, and clears the alert. (The integration tests start the real daemon,
# real git, real step hosts and the scripted agent; the disk reading is injected, never the real
# disk.)
set -euo pipefail
cd "$(dirname "$0")/../.."

run() {
    local what=$1; shift
    echo "== $what"
    out=$(cargo test -q "$@" 2>&1) || { echo "$out" >&2; echo "FAILED: $what" >&2; exit 1; }
    echo "$out" | grep -E '^test result' | tail -1
}

run "below the floor a task waits, the alert shows, then it lands and the alert clears" -p mason-daemon --test disk below_the_floor
run "at the floor a task runs" -p mason-daemon --test disk at_the_floor
run "the scheduler holds and explains" -p mason-scheduler disk
run "an old cancelled directory goes, a young one stays, a revived one is untouched" -p mason-daemon --test taskcache canceled
run "a revived task's directory is untouched" -p mason-daemon --test taskcache revived
run "the readers" -p mason-daemon --lib disk
echo "s85 OK"
