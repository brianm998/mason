#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1228 exit test: two owner notes with the same correction on different tasks are grouped, and the
# scripted review files one lesson that cites both.
set -euo pipefail
cd "$(dirname "$0")/../.."

grep -q '^Summary: ' docs/stages/s1228.md
cargo test -q -p mason-daemon --test it -- review::a_correction_on_two_tasks review::a_lesson_is_filed_once
echo "OK s1228"
