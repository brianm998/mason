#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s948 exit test: the daemon builds with base-kept-moving requeueing tasks.
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo check -q -p mason-daemon
test "$(grep -c "Contention, not the change's fault" crates/daemon/src/landing.rs)" = 2
echo "s948 OK"
