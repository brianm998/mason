#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
set -euo pipefail
cd "$(dirname "$0")/../.."
f=crates/conformance/src/main.rs
grep -q 'fn should_leave' $f
grep -q 'const QUIET_WINDOW_MS: u64 = 5000' $f
grep -q 'settle: quiet exit' $f
grep -q 'fn should_leave_waits_for_the_target_until_the_quiet_window' $f
grep -q '^Summary: ' docs/stages/s1081.md
echo "s1081 ok"
