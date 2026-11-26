#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1163 exit test: the file hold does not read the standard footer or the Trust section as
# files a task changes (decision 1153).
set -euo pipefail
cd "$(dirname "$0")/../.."

grep -q '^Summary: ' docs/stages/s1163.md
for t in a_body_names_its_files the_footer_names_no_files the_trust_section_names_no_files \
    a_plan_is_read_whole_and_an_unexplored_body_is_not_read \
    a_footer_overlap_does_not_hold_and_a_changed_file_still_does; do
    out=$(cargo test -q -p mason-daemon --lib -- --exact "file_hold::tests::$t" 2>&1) ||
        { echo "$out" >&2; echo "FAILED: $t" >&2; exit 1; }
    echo "$out" | grep -q '^test result: ok\. 1 passed' ||
        { echo "$out" >&2; echo "FAILED: $t did not run" >&2; exit 1; }
done
echo "s1163 OK"
