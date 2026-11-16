#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Stage 471 exit test: waiters short of test-suite do not reserve idle compile slots.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
cargo test -p mason-scheduler --test properties -- overnight waiters_short the_first_mixed an_exclusive_waiter
