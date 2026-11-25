#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1063 exit test: s899 follows the renamed 304 test; features check passes.
set -euo pipefail
cd "$(dirname "$0")/../.."
bash scripts/exit/s899.sh
grep -q 'fn a_304_changes_nothing_and_asks_for_nothing_more' crates/daemon/src/remote.rs
cargo test -q -p mason-daemon --lib -- a_304_changes_nothing_and_asks_for_nothing_more
cargo run -q -p mason-conformance -- features clients/features.toml
echo "s1063 OK"
