#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s95 exit test: a lost agent host is handled by settle_all, not reported as a crash.
set -euo pipefail
cd "$(dirname "$0")/../.."
grep -q "FailReason::HostLost { detail }" crates/adapters/src/runner.rs
grep -q "MAX_AGENT_RESTARTS" crates/daemon/src/jobs.rs
cargo test -q -p mason-daemon hostloss
echo ok
