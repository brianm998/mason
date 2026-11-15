#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s327 exit test: after fake CPU readings, a history query returns them in order with
# per-share values, and task shares carry their project.
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test -q -p mason-daemon --test protocol fake_cpu_readings_come_back_in_order
cargo test -q -p mason-daemon --lib cpuhistory
echo "s327: ok"
