#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s330 exit test: reminders queued while muted do not replay on unmute.
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test -q -p mason-store --lib -- a_later_round_replaces
cargo test -q -p mason-daemon --test it -- reminders_queued_while_muted
cargo test -q -p mason-daemon --test it -- a_never_heard_alert_plays_at_most_once
echo "s330 OK"
