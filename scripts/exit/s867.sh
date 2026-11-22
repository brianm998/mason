#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# `favor-remote` runs on GitHub until it is overloaded, then here under local slots (s867, decision 0860).
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
cargo test -q -p mason-daemon --lib favor_remote
cargo test -q -p mason-daemon --lib pickup_median
cargo test -q -p mason-daemon --lib an_unpicked_favor_remote_run
cargo test -q -p mason-daemon --test it favor_remote
echo "s867 ok"
