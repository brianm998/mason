#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s996 exit test: the local fallback counts the queue, settles the hold, takes its lease before
# giving up GitHub, and a withdrawn broker request ends its wait. Fake GitHub, fake readings.
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test -q -p mason-daemon --lib -- place::tests::room_ sched::tests::a_hold_counts sched::tests::a_try_never remote::tests::two_runs_at_the_timeout remote::tests::runs_resent_after remote::tests::the_pickup_clock remote::tests::an_unpicked_favor_remote
echo "s996 passed"
