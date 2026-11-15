#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s363 exit test: an agent's own `mason run` queue waits are added to its deadline.
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test -q -p mason-daemon --lib -- an_agents_mason_run_wait_is_added_to_its_deadline_when_granted
echo "s363 OK"
