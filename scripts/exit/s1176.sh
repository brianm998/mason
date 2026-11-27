#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1176 exit test: a cause outside the tasks' code is diagnosed once, with the facts MASON has,
# and the card pairs a fix-now with a fix-later action. The daemon tests use the fake adapter and
# a fake CI source:
#   the_1003_replay_holds_the_retry_and_the_card_files_the_fix_and_lands_now
#       (one diagnosis, the retry held, the prompt's log tail / "same on the base" / #1000,
#        the card answered: one task filed, Land now recorded with its debt)
#   a_file_task_that_repeats_an_open_ticket_is_refused_and_the_card_says_which
#   two_tasks_of_one_cause_get_one_diagnosis_and_one_card_that_acts_on_both
#   a_session_without_a_verdict_releases_the_held_retry
#   a_recommended_retry_releases_the_retry_and_waits_for_the_loop
#   the_fifth_cause_diagnosis_of_a_day_is_refused
#   a_ci_stuck_alert_starts_a_batch_diagnosis_while_the_batch_is_still_waiting
#       (a landing batch whose CI never finishes)
#   the_card_raises_a_diagnosis_alert_that_notify_toml_routes
# and the unit tests of the contract rule, the keys, the budget and the notice line.
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test -q -p mason-daemon --test it diagnose_cause::
cargo test -q -p mason-daemon --lib diagcause::
cargo test -q -p mason-adapter-api a_cause_outside_the_code
cargo test -q -p mason-core notify::
echo "s1176 OK"
