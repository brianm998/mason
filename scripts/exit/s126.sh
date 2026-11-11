#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s126 exit test: a frozen landing reaches the banner list with kind landing_frozen. The daemon's
# own landing test freezes a real landing and asserts the header carries that banner.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
cargo test -q -p mason-daemon --test landing a_red_post_land_tier_freezes_landing_until_the_fix_lands
echo "s126 OK"
