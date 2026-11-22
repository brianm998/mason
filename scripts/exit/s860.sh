#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# The develop deadline extender counts from the step's launch (s860, decision 0853).
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
cargo test -q -p mason-daemon --lib deadline
echo "s860 ok"
