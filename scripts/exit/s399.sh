#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s399 exit test: a fake agent's host is killed mid-session; the agent is resumed
# with --resume and the session id, and the loss note names the time.
set -euo pipefail
cd "$(dirname "$0")/../.."
grep -q "interrupted_note(lost_at)" crates/daemon/src/jobs.rs
cargo test -q -p mason-daemon --test host_loss a_killed_agent_host_is_resumed_and_the_task_lands
cargo test -q -p mason-daemon --lib hostloss
echo ok
