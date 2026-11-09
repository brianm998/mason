#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s76 exit test: a running step's tokens are live in its cell, stream events are not output,
# a restart does not double the total, and the clients' wording is covered.
set -euo pipefail
cd "$(dirname "$0")/../.."

run() {
    local what=$1; shift
    echo "== $what"
    out=$(cargo test -q "$@" 2>&1) || { echo "$out" >&2; echo "FAILED: $what" >&2; exit 1; }
    echo "$out" | grep -E '^test result' | tail -1
}

run "the parsers give live figures" -p mason-adapters --lib partial_messages
run "stream events are not output" -p mason-core --lib progress
run "the store keeps the tally" -p mason-store --test store
run "live tokens end to end" -p mason-daemon --test live_tokens

echo "s76 OK"
