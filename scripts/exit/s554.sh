#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s554 exit test: stuck:wait follows holder progress.
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test -q -p mason-core --lib -- health::
grep -q 'waits_holders' crates/daemon/src/sched.rs
echo "s554 OK"
