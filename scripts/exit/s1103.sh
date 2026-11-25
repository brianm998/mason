#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1103 exit test: the unchanged-view test allows for the usage clock (decision 1092).
set -euo pipefail
cd "$(dirname "$0")/../.."

grep -q '^Summary: ' docs/stages/s1103.md
grep -q 'usage identical to the last one sent' crates/daemon/tests/it/protocol.rs
cargo test -q -p mason-daemon --test it an_unchanged_view_is_not_sent_again_and_a_changed_one_is
echo "OK s1103"
