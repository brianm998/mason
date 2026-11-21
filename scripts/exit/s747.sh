#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s747 exit test: the landing window is waited out only while another task is moving.
set -euo pipefail
cd "$(dirname "$0")/../.."

grep -q '^Summary: ' docs/stages/s747.md || { echo "s747: stage note has no Summary line" >&2; exit 1; }
[ -f docs/decisions/0740-landing-window-needs-a-moving-task.md ] || { echo "s747: decision missing" >&2; exit 1; }
grep -q 'moving: usize' crates/landing/src/plan.rs || { echo "s747: form_batch has no moving input" >&2; exit 1; }
grep -q 'fn with_nobody_moving_the_window_is_not_waited_out' crates/landing/src/plan.rs || { echo "s747: test missing" >&2; exit 1; }
[ -f crates/daemon/tests/it/landing_collect.rs ] || { echo "s747: integration test missing" >&2; exit 1; }
echo "s747: ok"
