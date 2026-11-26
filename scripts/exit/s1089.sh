#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1089 exit test: an unchanged view is not sent again, though the usage text follows the clock
# (decision 1078).
set -euo pipefail
cd "$(dirname "$0")/../.."

grep -q '^Summary: ' docs/stages/s1089.md
cargo test -q -p mason-daemon --test it protocol::an_unchanged_view_is_not_sent_again_and_a_changed_one_is
echo "OK s1089"
