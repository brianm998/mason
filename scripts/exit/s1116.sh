#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1116 exit test: a failed task holds its diagnosis card and the owner answers it like any other.
# The daemon tests use the fake adapter:
#   a_failed_task_holds_the_card_until_it_is_dismissed_or_retried  (card, actions, 409, 403, 400)
#   a_retry_option_retries_with_the_note_and_the_context
#   the_explore_close_and_cancel_options_do_what_they_say
#   other_words_retry_from_the_default_step_as_the_owner_wrote_them
#   context_on_an_open_question_reaches_the_next_round
#   the_cli_answers_a_failed_tasks_card_with_context  (a real masond: fail a task, wait for the
#   diagnosis card, `mason task answer 0 2 --context`, read the next prompt)
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test -q -p mason-daemon --test it diagnose_answer::
echo "s1116 OK"
