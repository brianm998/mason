#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s807 exit test: the coverage map is made on GitHub when remote is on, and `mason doctor`
# says when llvm-tools is missing. Runs the tests for both.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
grep -q '^Summary: ' docs/stages/s807.md
cargo test -q -p mason-daemon --lib -- \
  a_due_map_pushes_one_covmap_run a_map_of_another_commit_is_not_kept
cargo test -q -p mason-pipeline -- doctor_reports_missing_llvm_tools
echo ok
