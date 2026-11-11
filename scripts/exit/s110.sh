#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s110 exit test: a frozen base raises an alert.
#  1. a freeze row: the alert appears, names the fix task, changes wording when the fix
#     task fails, and clears with the freeze;
#  2. a project with a post-land tier whose suite goes red: the alert shows the freeze and
#     its fix task, and clears once the fix lands and the suite is green.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
t() {  # TEST-FILE TEST-NAME
  cargo test -q -p mason-daemon --test "$1" -- --exact "$2" 2>&1 | tail -3 | grep -q 'test result: ok. 1 passed' \
    || { echo "FAILED: $1::$2" >&2; cargo test -p mason-daemon --test "$1" -- --exact "$2" 2>&1 | tail -25 >&2; exit 1; }
}
echo "== 1. the alert, its wording, its clearing"
t attention a_frozen_base_raises_an_alert_naming_the_fix_task_until_the_freeze_clears
echo "== 2. a red post-land suite freezes the base; the alert follows it"
t landing a_red_post_land_tier_freezes_landing_until_the_fix_lands
echo "s110 OK"
