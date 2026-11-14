#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s261 exit test: stopping a step also stops descendants that left the command's process group.
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test -q -p mason-platform --lib -- descendants pid_one reused_pid the_table_parses
cargo test -q -p mason-step-host --test host -- descendant
echo "s261 OK"
