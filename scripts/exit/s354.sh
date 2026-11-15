#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s354 exit test: a long interactive command is one short line with the whole command beside it.
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test -q -p mason-scheduler --lib -- describe_command_cases label_parts_split
cargo test -q -p mason-scheduler --lib -- a_long_interactive_holder_is_cut
cargo test -q -p mason-protocol -- a_compile_row_carries_its_command
cargo test -q -p mason-daemon --test scheduling -- a_long_interactive_script_is_one_short_line
echo "s354 OK"
