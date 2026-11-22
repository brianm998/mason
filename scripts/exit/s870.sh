#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s870 exit test: a GitHub run nobody will collect is cancelled at once (#823).
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
cargo test -p mason-daemon --lib -- \
  a_superseded_check_is_cancelled_queued_or_in_progress_and_its_hold_is_gone \
  the_reaper_names_a_check_whose_record_says_superseded_or_delivered \
  a_check_is_told_to_stop_once_its_run_is_no_longer_the_open_one
echo "s870 passed"
