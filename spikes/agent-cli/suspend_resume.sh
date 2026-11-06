#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Spike 1: can a turn end on a pending check and resume cleanly with the result,
# and what does the prompt cache do across waits? Uses haiku on a scratch dir.
# Usage: suspend_resume.sh [WAIT_SECONDS ...]   (default waits: 0 330)
set -euo pipefail
source "$(dirname "$0")/lib.sh"; need_login
waits=("${@:-0 330}"); [ "$#" -eq 0 ] && waits=(0 330)
echo "== turn 1: call run_check, expect the turn to end"
out=$(claude_run "Call the run_check tool with name 'unit'. Then follow the instruction in its result exactly."); echo "$out" | summ
sid=$(echo "$out" | python3 -c 'import json,sys;print(json.load(sys.stdin)["session_id"])')
n=2
for w in "${waits[@]}"; do
  echo "== waiting ${w}s"; sleep "$w"
  echo "== turn $n: resume with result (wait was ${w}s)"
  claude_run --resume "$sid" "RESUME NOTE: check 'unit' finished: green, 12 tests executed. Reply with RESULT_OK and the test count, and what check you started earlier." | summ
  n=$((n+1))
done
