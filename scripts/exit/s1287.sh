#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1287 exit test: an agent CLI that cannot start holds the project with a notice instead of
# failing its tasks. Real daemons with the fake agent:
#  1. A claude_bin that does not exist: the task stays queued with the notice and never fails;
#     pointed at the fake agent and reloaded, it runs and lands.
#  2. A launch that ends in "failed to start" is not retried or counted, and a diagnosis session
#     that cannot start spends none of the budget.
#  3. `mason doctor` reports what the running daemon resolves.
set -euo pipefail
cd "$(dirname "$0")/../.."

grep -q '^Summary: ' docs/stages/s1287.md
test -f docs/decisions/1277-agent-cli-hold.md
cargo build -q --workspace --bins
cargo test -q -p mason-daemon --test it agent_bin::
cargo test -q -p mason-daemon --lib -- agentbin:: diagnose::
cargo test -q -p mason-adapters --lib -- start_failure_tests::
cargo test -q -p mason-pipeline --bin mason -- doctor::
echo "OK s1287"
