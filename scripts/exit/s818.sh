#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# favor-local runs here until this machine is overloaded, then spills to GitHub (s818, decision 0811).
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
cargo test -q -p mason-daemon --lib favor_local
cargo test -q -p mason-daemon --lib a_queued_local_run_moves
cargo test -q -p mason-daemon --test it favor_local_spills_to_github
echo "s818 ok"
