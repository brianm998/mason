#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Stage 1032 exit test: the demo daemon resends views, and s13 passes under CPU load.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
cargo test -p mason-daemon --test it a_demo_client_gets_all_three_views
pids=()
for _ in 1 2 3 4; do yes > /dev/null & pids+=($!); done
trap 'kill "${pids[@]}" 2>/dev/null || true' EXIT
for _ in 1 2 3; do MASON_NO_GUI=1 bash scripts/exit/s13.sh; done
echo "s1032 passed"
