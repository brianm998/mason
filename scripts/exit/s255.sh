#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s255 exit test: the digest has, per project and in --json, the p90 beside the median claim-to-land
# time, send-backs per landed task by reason class, retries by failure class (by hand and
# automatic), the test step's run times, and says "not recorded" for what is not. The aggregation
# runs over a synthetic event list.
set -euo pipefail
cd "$(dirname "$0")/../.."

run() {
    local what=$1; shift
    echo "== $what"
    out=$(cargo test -q "$@" 2>&1) || { echo "$out" >&2; echo "FAILED: $what" >&2; exit 1; }
    echo "$out" | grep -E '^test result' | tail -3
}

run "p90 and the send-back classes" -p mason-core --lib -- introspect
run "the per-project numbers over a synthetic event list" -p mason-daemon --lib -- the_digest_counts_send_backs
echo "s255 OK"
