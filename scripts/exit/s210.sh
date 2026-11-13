#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s210 exit test: run_check can be narrowed while developing; the test step still runs it all.
#  1. Scope names are validated (no shell metacharacters) and reach the handler.
#  2. Which packages a change touches (plus dependents) on a small fixture workspace.
#  3. dev_command / dev_build parse and validate.
#  4. A daemon test: a develop agent's scoped run_check runs the filtered command (tests) and the
#     dev command (packages), and the test step still runs the whole suite.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
run() {
    local what=$1; shift
    echo "== $what"
    out=$(cargo test -q "$@" 2>&1) || { echo "$out" >&2; echo "FAILED: $what" >&2; exit 1; }
    echo "$out" | grep -E '^test result' | tail -1
}
run "1. scope validation and hand-over" \
    -p mason-adapter-api a_scope_reaches_the_handler_and_its_names_are_never_shell_text
run "2. packages a change touches" -p mason-daemon --lib scope::
run "3. dev_command and dev_build parse and validate" -p mason-definition --test testing_tables
run "4. daemon: scoped run_check, whole test step" \
    -p mason-daemon --test suite_builds a_scoped_run_check_runs_the_narrowed_command_and_the_test_step_still_runs_it_all
echo "s210 OK"
