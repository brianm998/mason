#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s428 exit test: a passed build is remembered per tree and worktree; wait evidence names the longest wait.
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test -q -p mason-daemon --test testing -- a_passed_build_is_remembered
cargo test -q -p mason-core --lib -- the_wait_evidence_names_the_longest_wait
echo "s428 OK"
