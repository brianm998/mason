#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s62 exit test: every adapter reports usage in one shape, from recorded streams.
set -euo pipefail
cd "$(dirname "$0")/../.."

run() {
    local what=$1; shift
    echo "== $what"
    out=$(cargo test -q "$@" 2>&1) || { echo "$out" >&2; echo "FAILED: $what" >&2; exit 1; }
    echo "$out" | grep -E '^test result' | tail -1
}

run "real captures, synthetic blocks, overage, api spend, unknown windows, Codex text" \
    -p mason-adapters --test usage
run "the runner keeps the latest snapshot, stamped" \
    -p mason-adapters --test runner usage
run "conformance: fake and Claude stand-in pass; broken usage rules are caught" \
    -p mason-adapters --test conformance

echo "s62 OK"
