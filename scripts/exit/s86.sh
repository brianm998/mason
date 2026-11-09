#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s86 exit test: a real masond with the scripted agent and a panic injected in a landing
# batch: the task fails with the reason, `mason status` shows the bug, and a second task
# lands without a restart. The other containment tests run with it.
set -euo pipefail
cd "$(dirname "$0")/../.."

cargo test -q -p mason-store --test store panic 2>&1 | tail -3
cargo test -q -p mason-daemon --test panics 2>&1 | tail -5
echo "s86 OK"
