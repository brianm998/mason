#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1306 exit test: the diagnosis card tests answer by label, whatever order the card lists.
#   a_retry_option_retries_with_the_note_and_the_context
#   the_explore_close_and_cancel_options_do_what_they_say
# and the rest of the diagnose_answer class with them.
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test -q -p mason-daemon --test it diagnose_answer::
echo "s1306 OK"
