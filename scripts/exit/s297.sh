#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s297 exit test: a fake agent asks a multi-select question, the owner answers
# with two options ("1,3"), and the agent's resumed note holds both labels. The
# card, the list parsing and the CLI's answer argument are checked too.
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test -q -p mason-daemon --test it -- a_multi_select
cargo test -q -p mason-adapter-api --lib -- multi_select pick_many
cargo test -q -p mason-pipeline --bin mason -- a_list_of_options_is_one_answer
echo "s297 OK"
