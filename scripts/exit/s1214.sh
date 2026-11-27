#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1214 exit test: file edits are parsed, timelined and summarised (decision 1204).
set -euo pipefail
cd "$(dirname "$0")/../.."

cargo test -q -p mason-adapters --lib edit_write_and_multiedit_calls
cargo test -q -p mason-daemon --lib edits::
cargo test -q -p mason-daemon --lib the_edit_history_block
echo "OK s1214"
