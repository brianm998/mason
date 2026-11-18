#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s551 exit test: a scripted CI watch over a fake source. Run R1 is red and filed once, its
# task is closed, ten passes with the clock past red_ticket_after leave exactly one task for
# R1, and a red R2 files exactly one more.
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test -q -p mason-daemon --test it ci_watch::ten_passes_past_the_threshold
cargo test -q -p mason-core ci_watch
echo "s551: ok"
