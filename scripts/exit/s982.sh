#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Add Project endpoints: candidates, plan, apply, owner only (s982, 0975).
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
cargo test -q -p mason-daemon --test it -- onboarding::
echo "s982: all checks passed"
