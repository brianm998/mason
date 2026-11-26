#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
set -euo pipefail
cd "$(dirname "$0")/../.."
f=crates/conformance/src/main.rs
# The expected capturer waits for the step's seq exactly, and fails loudly (decision 1025).
grep -q 'fn reached' $f
grep -q '!demo && views >= 3' $f
grep -q 'fn a_demo_wait_leaves_only_once_the_target_is_reached' $f
grep -q 'did not reach the daemon.s seq {target} within 60 s' $f
grep -q 'fn settle_target_needs_the_steps_seq_and_three_views' $f
grep -q '^Summary: ' docs/stages/s1081.md
echo "s1081 ok"
