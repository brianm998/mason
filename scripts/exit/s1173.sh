#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1173 exit test: the hold sentences name the task, file, count and limit, never an LLM session.
set -euo pipefail
cd "$(dirname "$0")/../.."
out=$(cargo test -q -p mason-daemon --lib -- file_hold::tests::the_file_hold_sentence file_hold::tests::the_check_hold_sentence file_hold::tests::a_file_hold_sentence 2>&1) || { echo "$out" >&2; exit 1; }
echo "$out" | grep -E '^test result' | tail -3
echo "s1173 OK"
