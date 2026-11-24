#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1043 exit test (decision 1036): a batch whose CI run stops changing raises a `landing` alert.
#  1. The pure rule (ci_stuck): unchanged past the limit, reset by a change, finished, queued-only, off.
#  2. A daemon with a scripted CI that stays pending with the same running and queued jobs and
#     `stuck_after = "1s"`: the alert (the one `mason alerts` lists) names the batch and the
#     running job; once the scripted CI is released to green, or to red, the alert is gone.
set -euo pipefail
cd "$(dirname "$0")/../.."

grep -q '^Summary: ' docs/stages/s1043.md
echo "== 1. the rule"
cargo test -q -p mason-core --lib -- ci_stuck::
echo "== 2. the alert, raised and cleared"
cargo test -q -p mason-daemon --test it -- ci_stuck::
echo "OK s1043"
