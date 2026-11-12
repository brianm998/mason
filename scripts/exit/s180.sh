#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s180 exit test: a probe that gets through lifts the hold before its session ends.
set -euo pipefail
cd "$(dirname "$0")/../.."
grep -q "pub fn probe_ok" crates/daemon/src/holds.rs
grep -q "on_live" crates/adapters/src/runner.rs
cargo test -q -p mason-daemon --test holds a_probe_that_gets_through_lifts_the_hold_before_its_session_ends
echo ok
