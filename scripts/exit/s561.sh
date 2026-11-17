#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s561 exit test: 500 history tasks, no activity; a subscribed client gets the sidebar once over
# several rebuilds, and without bodies it is under a quarter of the size with them.
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test -q -p mason-daemon --test protocol history_bodies_come_with_row_bodies_and_the_sidebar_is_sent_once
cargo test -q -p mason-daemon --test row_bodies
echo "s561: ok"
