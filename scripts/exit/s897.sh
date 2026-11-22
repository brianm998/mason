#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s897 exit test: the source-free note test passes.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
cargo test -p mason-daemon a_diff_with_none_of_the_planned_source_files_is_refused_once_and_accepted_with_a_note 2>&1 | tee /dev/stderr | grep -q '1 passed'
