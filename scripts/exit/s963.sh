#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s963 exit test: a deleted task branch's debt is still proven landed from the base's merge.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
cargo test -p mason-daemon --lib merged_branch_tests
