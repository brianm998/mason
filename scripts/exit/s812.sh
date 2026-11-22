#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# A run that `fallback = "local"` sends here from GitHub takes the machine slots it would hold
# when placed here, so the slot limits and the CPU and thermal holds apply (s812, decision 0805).
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
cargo test -q -p mason-daemon --test it fallback_lease
echo "s812 ok"
