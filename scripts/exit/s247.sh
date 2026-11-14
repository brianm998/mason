#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s247 exit test: a failure outside the code is retried by the daemon itself (twice, then it stays
# failed), a failure about the code never is, and three tasks failing the same way raise one alert.
# A fake remote whose pre-receive hook rejects pushes makes the tasks fail at landing.
set -euo pipefail
cd "$(dirname "$0")/../.."

run() {
    local what=$1; shift
    echo "== $what"
    out=$(cargo test -q "$@" 2>&1) || { echo "$out" >&2; echo "FAILED: $what" >&2; exit 1; }
    echo "$out" | grep -E '^test result' | tail -3
}

run "the failure classifier" -p mason-core --lib fail_class
run "the retry schedule and the burst grouping" -p mason-daemon --lib autoretry
run "the daemon retries by itself" -p mason-daemon --test auto_retry
echo "s247 OK"
