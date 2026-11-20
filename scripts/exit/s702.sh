#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s702 exit test: a batch whose fake suite hangs once lands after one rerun; one that hangs
# under a single task's change holds that task back after filtered runs.
set -uo pipefail
cd "$(dirname "$0")/../.."
cargo test -q -p mason-daemon --test it landing_timeout:: 2>&1 | tail -15
exit "${PIPESTATUS[0]}"
