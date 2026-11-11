#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s129 exit test: the home list is driven by the broker's granted LLM leases.
set -euo pipefail
cd "$(dirname "$0")/../.."
grep -q 'fn llm_held_by' crates/daemon/src/sched.rs
grep -q 'llm_held_by' crates/daemon/src/views.rs
grep -q 'active_models_lists_only_tasks_that_hold_a_session' crates/daemon/tests/scheduling.rs
echo "s129: ok"
