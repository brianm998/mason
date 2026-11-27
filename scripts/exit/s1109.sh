#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1109 exit test: what earlier rounds tried and gave up reaches the next round, fenced and
# newest first (task 1128, decision 1098).
set -euo pipefail
cd "$(dirname "$0")/../.."

grep -q '^Summary: ' docs/stages/s1109.md
cargo test -q -p mason-adapter-api --lib -- contract::tests::tried_
cargo test -q -p mason-daemon --lib -- tried prompts::tests diagnose::tests::the_prompt_carries
cargo test -q -p mason-pipeline --bins -- task_show_lists_what_a_run_tried
cargo test -q -p mason-daemon --test it -- what_earlier_rounds_tried
echo "OK s1109"
