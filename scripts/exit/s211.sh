#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s211 exit test: definition pinning across send-backs and retries (decision 0206).
#  a. a task sent back after a new definition landed runs its next steps under it, with a repin event
#  b. a running step keeps the definition it started with
#  c. a red in a suite that changed is rerun once on the current definition, with no send-back
#  d. a red in a suite that did not change is sent back
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
run() {
    local what=$1; shift
    echo "== $what"
    out=$(cargo test -q "$@" 2>&1) || { echo "$out" >&2; echo "FAILED: $what" >&2; exit 1; }
    echo "$out" | grep -E '^test result' | tail -1
}
run "a+b. send-back repins; a running step keeps its pin" \
    -p mason-daemon --test repin a_task_sent_back_runs_the_definition_on_the_base_now
run "c. a red in a changed suite is rerun once" \
    -p mason-daemon --test repin a_red_in_a_suite_that_changed_is_rerun_once
run "d. a red in an unchanged suite is sent back" \
    -p mason-daemon --test repin a_red_in_a_suite_that_did_not_change
run "note for .mason/ changes" \
    -p mason-daemon --test repin only_a_project_without
run "red reason parsing" \
    -p mason-daemon --test repin the_suite_a_red_names
run "the event round-trips" \
    -p mason-store every_event_kind_round_trips
run "reducer matrix" -p mason-engine --test matrix
echo "s211 OK"
