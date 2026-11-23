#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1000 exit test: the diagnosis session's model and effort are configurable and default to
# Opus at medium; `mason task diagnosis` and `mason task show` print what it ran on.
set -euo pipefail
cd "$(dirname "$0")/../.."

run() {
    local what=$1; shift
    echo "== $what"
    out=$(cargo test -q "$@" 2>&1) || { echo "$out" >&2; echo "FAILED: $what" >&2; exit 1; }
    echo "$out" | grep -E '^test result' | tail -3
}

run "the spec, precedence and effort check" -p mason-daemon --lib -- diagnose_session_spec diagnose_effort diagnose_bad_effort diagnose_builtin
# The CLI prints the `diagnosis: <model>, <effort> (<source>)` line the daemon stores per row.
grep -q 'println!("    diagnosis: {r}")' crates/cli/src/main.rs || { echo "FAILED: no diagnosis line in mason task diagnosis" >&2; exit 1; }
grep -q 'println!("  diagnosis: {r}")' crates/cli/src/main.rs || { echo "FAILED: no diagnosis line in mason task show" >&2; exit 1; }
grep -q 'diagnose_text(&entry, from)' crates/daemon/src/diagnose.rs || { echo "FAILED: ran_on text not built" >&2; exit 1; }
echo "s1000 OK"
