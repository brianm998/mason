#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1120 exit test: a check a node gives back runs under the compile slot its task holds, with
# compile 1 and 2, while a step start keeps waiting; what it lacks is ranked ahead of step
# starts; a self-deadlock is alerted and explained (#1149). Scratch daemons, a paired scratch
# node whose job is cancelled, no GitHub.
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test -q -p mason-scheduler --lib -- deadlock::tests rank::tests::a_request_that_holds
cargo test -q -p mason-daemon --test it -- fallback_lease::
cargo test -q -p mason-daemon --test nodes -- a_check_the_node_gives_back
grep -q '^Summary: ' docs/stages/s1120.md
echo "s1120 passed"
