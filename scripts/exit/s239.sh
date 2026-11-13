#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s239 exit test: the pause between compile starts applies only while a compile is running.
# compile_settle_s is 600 and the CPU reads low (a fake source; no real load).
#  1. One compile starts and finishes at once; the next is granted at once, not after 600 s.
#  2. With a compile held, a further one waits "letting the last compile show in CPU use".
#  3. The planner: none held grants the first; one held still pauses; two in a pass pace the second.
#  4. A request behind a paused one names it and the reason, not a holder.
# No timing assertions: the clock is injected and the broker test reads state, not sleeps.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
run() {
    local what=$1; shift
    echo "== $what"
    out=$(cargo test -q "$@" 2>&1) || { echo "$out" >&2; echo "FAILED: $what" >&2; exit 1; }
    echo "$out" | grep -E '^test result' | tail -1
}
run "1+2. broker: idle start, paused beside a running compile" \
    -p mason-daemon --test limits the_pause_between_compile_starts
run "3+4. planner and wait text" -p mason-scheduler
echo "s239 OK"
