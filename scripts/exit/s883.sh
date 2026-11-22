#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s883 exit test: the active-models test is still there and opens the other session's lease first (0831).
set -euo pipefail
cd "$(dirname "$0")/../.."
f=crates/daemon/tests/scheduling.rs
grep -q 'active_models_lists_only_tasks_that_hold_a_session' "$f"
grep -q 'take_when_granted(&d, id, "other session")' "$f"
echo "s883: ok"
