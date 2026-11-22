#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s883 exit test: the active-models test expects waiting_slot rows (0747).
set -euo pipefail
cd "$(dirname "$0")/../.."
f=crates/daemon/tests/scheduling.rs
grep -q 'active_models_lists_only_tasks_that_hold_a_session' "$f"
grep -q 'fn wait_active' "$f"
grep -q 'waiting_slot' "$f"
echo "s883: ok"
