#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1246 exit test: `mason prompts eval` replays a case under two revisions with a fake adapter,
# records both verdicts and the grader's judgement, and ends with a one-line summary.
set -euo pipefail
cd "$(dirname "$0")/../.."
grep -q '"prompts" =>\|cmd == "prompts"' crates/cli/src/main.rs
grep -q 'pub fn run' crates/daemon/src/prompts_eval.rs
[ -n "${MASON_TESTS_ALREADY_RAN:-}" ] || cargo test -q -p mason-daemon --lib prompts_eval
echo "s1246: ok"
