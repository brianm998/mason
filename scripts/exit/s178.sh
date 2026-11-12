#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s178 exit test: suites with a build, an impact map, and run_check on the selected suites.
# Stand-in suites only (no Swift, no Gradle). The project has two suites, each with a `build`.
#  1. A change under the second suite's path runs only that suite (the first is "not run").
#  2. The broker (what `mason resources` shows) has `compile` held during the build and
#     released before the run, which holds only the suite's own `uses`.
#  3. The stand-in agent's run_check result lists only the selected suite.
#  4. A suite's build parses and validates; MASON's own definition validates and its impact
#     map picks suites by path.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
run() {
    local what=$1; shift
    echo "== $what"
    out=$(cargo test -q "$@" 2>&1) || { echo "$out" >&2; echo "FAILED: $what" >&2; exit 1; }
    echo "$out" | grep -E '^test result' | tail -1
}
run "1+3. run_check and the test step run only the suite the change's path selects" \
    -p mason-daemon --test suite_builds run_check_runs_only_the_development_suites_the_impact_map_selects
run "2. compile is held during the build and released before the run; one build is shared" \
    -p mason-daemon --test suite_builds a_build_holds_compile_and_the_run_holds_only_the_suites_own_uses
run "no development binding: run_check keeps the single command" \
    -p mason-daemon --test suite_builds run_check_without_a_development_binding_keeps_the_single_command
run "4. a suite's build parses and validates; MASON's own impact map" \
    -p mason-definition --test testing_tables
echo "s178 OK"
