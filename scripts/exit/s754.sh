#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s754 exit test: home shows what the agents are doing and what is out on GitHub (#763).
# Runs the new tests by name. The Swift test runs where the Swift toolchain is.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
cargo test -q -p mason-daemon --test scheduling -- an_agent_whose_check_is_on_github
cargo test -q -p mason-cli -- agents_and_github_lines
if command -v swift >/dev/null 2>&1 && [ -d clients/swift ]; then
  (cd clients/swift && swift test --filter testActiveHeaderCountsWorkingAndWaiting)
fi
echo "s754 passed"
