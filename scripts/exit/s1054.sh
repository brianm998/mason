#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1054 exit test (multi-machine nodes, phase 0): a third executor has room at every match site
# and `decide` asks two questions, with the decisions unchanged.
set -euo pipefail
cd "$(dirname "$0")/../.."

grep -q '^Summary: ' docs/stages/s1054.md
grep -q 'Node {' crates/daemon/src/remote.rs
grep -q 'Node {' crates/daemon/src/place.rs
grep -q 'fn may_leave' crates/daemon/src/place.rs
# The existing decision tables and the new rows for the split.
cargo test -q -p mason-daemon --lib place::
echo "OK s1054"
