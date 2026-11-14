#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s254 exit test: an explorer can close a task with `submit_close` (already done, a duplicate or
# not a problem, with evidence). The task waits in a veto window as a decision card ("Close #N:
# <kind>: <evidence>", options Close now / Reopen and explore): 24 hours for the owner's own tasks,
# 2 hours for tasks MASON or its agents filed. With no veto the task is closed with no change;
# "Close now" closes it at once; "Reopen and explore" sends it back to exploring. The wait is in the
# store, so a restart keeps it.
set -euo pipefail
cd "$(dirname "$0")/../.."

run() {
    local what=$1; shift
    echo "== $what"
    out=$(cargo test -q "$@" 2>&1) || { echo "$out" >&2; echo "FAILED: $what" >&2; exit 1; }
    echo "$out" | grep -E '^test result' | tail -3
}

run "submit_close is validated and is an explore terminal" -p mason-adapter-api --lib -- a_close_needs
run "the card and the closing reason" -p mason-daemon --lib -- closer
echo "s254 OK"
