#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1012 exit test: a replayed run_check takes the results of the suites that finished before a
# takeover (held until the agent has the answer), and runs nothing twice.
set -euo pipefail
cd "$(dirname "$0")/../.."

grep -q '^Summary: ' docs/stages/s1012.md
cargo test -q -p mason-daemon --test it -- a_held_result_is_reused_by_a_replay a_call_without_a_hold
echo "OK s1012"
