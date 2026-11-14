#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s252 exit test: the weakening scan refuses a new timing-dependent test (a fixed sleep then an
# assert, an assert on elapsed time against a constant, a sub-second fixed timeout as
# synchronisation, in Rust, Kotlin and Swift test code), lets a polling loop and a line carrying an
# unexpired `// timing: owner <name> expires <date>` marker through, and names file, line and rule.
set -euo pipefail
cd "$(dirname "$0")/../.."

run() {
    local what=$1; shift
    echo "== $what"
    out=$(cargo test -q "$@" 2>&1) || { echo "$out" >&2; echo "FAILED: $what" >&2; exit 1; }
    echo "$out" | grep -E '^test result' | tail -3
}

run "each pattern, the marker and the false positives" -p mason-daemon --lib -- protect::timing
run "a timing-dependent test in a diff is a finding" -p mason-daemon --lib -- a_timing_dependent_test_in_a_diff
echo "s252 OK"
