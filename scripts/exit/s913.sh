#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s913 exit test: the source-free note test keeps its note outside the repository.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
cargo test -q -p mason-daemon --lib -- a_diff_with_none_of_the_planned_source_files
echo "s913 passed"
