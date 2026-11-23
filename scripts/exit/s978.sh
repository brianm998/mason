#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s978 exit test: a guard's `<path>: <message>` lint line parks the task that added the path,
# with no bisect (the landing integration test runs the batch against a scripted lint).
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
cargo test -q -p mason-daemon --lib lint_files_reads_guard_lines_only_for_files_in_the_diff
cargo test -q -p mason-daemon --test it -- a_guard_line_naming_a_file_one_task_added
echo "s978 ok"
