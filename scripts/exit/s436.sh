#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s436 exit test: the cpuhistory minute-bucket test passes.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
cargo test -q -p mason-daemon --lib cpuhistory::tests::minute_buckets_keep_totals_within_rounding
echo "s436 OK"
