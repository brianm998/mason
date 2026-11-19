#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s642 exit test: the landing fast-tier tests pass five runs in a row.
set -euo pipefail
cd "$(dirname "$0")/../.."
for i in 1 2 3 4 5; do
    echo "== run $i"
    cargo test -q -p mason-daemon --test landing_fast_tier || { echo "FAILED: run $i" >&2; exit 1; }
done
