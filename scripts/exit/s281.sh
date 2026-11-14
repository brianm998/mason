#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s281 exit test: every adapter tool has a spec and belongs to at least one step (#308).
set -euo pipefail
cd "$(dirname "$0")/../.."

run() {
    local what=$1; shift
    echo "== $what"
    out=$(cargo test -q "$@" 2>&1) || { echo "$out" >&2; echo "FAILED: $what" >&2; exit 1; }
    echo "$out" | grep -E '^test result' | tail -3
}

run "every tool has a spec and is in a step" -p mason-adapter-api --lib -- every_tool_has_a_spec_and_belongs_to_some_step
run "submit_diagnosis is validated" -p mason-adapter-api --lib -- diagnosis
echo "s281 OK"
