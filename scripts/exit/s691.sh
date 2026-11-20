#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s691 exit test: landing lints before the batch suite and blames a lint red by file.
set -uo pipefail
cd "$(dirname "$0")/../.."
if cargo test -q -p mason-daemon --test it landing::lint_runs_before_the_batch_suite >/dev/null 2>&1 \
  && cargo test -q -p mason-daemon --test it landing::a_lint_red_in_a_file_no_task_touches >/dev/null 2>&1 \
  && cargo test -q -p mason-conformance --test ci_gates the_test_step_has_a_fast_fmt_suite >/dev/null 2>&1; then
  echo "PASS s691"
else
  echo "FAIL s691"
  exit 1
fi
