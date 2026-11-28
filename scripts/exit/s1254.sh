#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1254 exit test: with a scratch node of one slot held by a long check, a nodes-only suite waits
# in line and goes before a later check, and the sentence names holder, place and estimate.
set -euo pipefail
cd "$(dirname "$0")/../.."

grep -q '^Summary: ' docs/stages/s1254.md
cargo test -q -p mason-daemon a_suite_waiting_for_a_full_node_is_in_line_and_goes_before_later_work
cargo test -q -p mason-daemon nodewait::
cargo test -q -p mason-daemon the_pool_is_never_more_than_the_nodes_own_jobs
grep -q 'node_waits' crates/cli/src/main.rs
echo "OK s1254"
