#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s772 exit test: the agentshim tests still run and pass on unix (#743).
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
cargo test -q -p mason-daemon --lib -- agentshim
echo "s772 passed"
