#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1243 exit test: with a scratch coordinator and node and fake exit scripts, a sharded suite on the
# node runs each script once, and a rerun after an exhausted budget finishes the remainder
# (decision 1233).
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test -q -p mason-daemon --test nodes a_sharded_suite_on_a_node
cargo test -q -p mason-daemon --test nodes a_rerun_off_this_mac_skips
cargo test -q -p mason-daemon --test exit_runner_budget a_runner_off_this_mac
cargo test -q -p mason-daemon --lib node_split::
