#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s552 exit test: a waiting exclusive request no longer idles free slots. With three long
# holders overrun, an exclusive request and waiters behind it, the waiters are granted with
# nothing finishing, and the exclusive request goes first once the holders end. A non-person
# `--exclusive` is bounded to all compile units.
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test -q -p mason-scheduler --test properties -- the_0922 holders_that_overran an_exclusive_request_gives_up
cargo test -q -p mason-daemon --test origin_rank
echo "s552: ok"
