#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1112 exit test: only a blocking review finding sends a task back. A real daemon with a
# scripted reviewer: an approval with two minor findings lands and the task lists both as
# minor; "changes" with only minor findings is refused and the corrected submission accepted.
set -euo pipefail
cd "$(dirname "$0")/../.."

grep -q '^Summary: ' docs/stages/s1112.md

run() {
    local what=$1; shift
    echo "== $what"
    out=$(cargo test -q "$@" 2>&1) || { echo "$out" >&2; echo "FAILED: $what" >&2; exit 1; }
    echo "$out" | grep -E '^test result: ok\. [1-9]' || { echo "$out" >&2; echo "FAILED: $what ran no test" >&2; exit 1; }
}

run "the contract" -p mason-adapter-api --lib -- only_a_blocking_finding_can_ask_for_changes
run "the review prompt" -p mason-daemon --lib -- the_review_prompt_says_what_is_blocking
run "the kept findings" -p mason-daemon --lib -- reviewfindings
run "scripted review sessions" -p mason-daemon --test it -- review_severity::
echo "s1112 OK"
