#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s244 exit test: the outside-holder sched test passes alone and in the full lib run.
set -euo pipefail
cd "$(dirname "$0")/../.."

run() {
    local what=$1; shift
    echo "== $what"
    out=$(cargo test -q "$@" 2>&1) || { echo "$out" >&2; echo "FAILED: $what" >&2; exit 1; }
    echo "$out" | grep -E '^test result' | tail -1
}

run "a slot held outside the broker is not granted again" \
    -p mason-daemon --lib a_slot_held_outside_the_broker_is_not_granted_again
run "the full daemon lib run" -p mason-daemon --lib
echo "s244 OK"
