#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s980 exit test: a task's green count is not another task's expected total; a count past an
# expected total raises it.
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test -q -p mason-store suite_size_is_not_overwritten_by_task_greens
cargo test -q -p mason-daemon --lib a_count_past_an_expected_total_raises_it
echo "s980: ok"
