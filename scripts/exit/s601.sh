#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s601 exit test: the two formerly red daemon tests pass.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
cargo test -p mason-daemon --test it -- health::a_direct_fix notify::one_alert
