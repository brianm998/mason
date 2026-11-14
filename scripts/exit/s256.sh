#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s256 exit test: a project's next queued task is not started while more than its limit of
# run_check and test-suite requests wait (twice the test-suite slots by default, or
# `[admission] max_check_queue`, 0 = off); an urgent task is never held; the held task's activity says
# "held: N checks are waiting (limit M)". The decision and the definition key are unit tests.
set -euo pipefail
cd "$(dirname "$0")/../.."

run() {
    local what=$1; shift
    echo "== $what"
    out=$(cargo test -q "$@" 2>&1) || { echo "$out" >&2; echo "FAILED: $what" >&2; exit 1; }
    echo "$out" | grep -E '^test result' | tail -3
}

run "the admission decision" -p mason-daemon --lib -- admission_tests
run "the definition key" -p mason-definition --lib -- an_admission_table
echo "s256 OK"
