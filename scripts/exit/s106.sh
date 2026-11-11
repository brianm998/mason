#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s106 exit test: a real masond with a panic injected in a background loop shows a
# `daemon_bug` alert in `mason alerts`, and the loop goes on. The pure rules run with it.
set -euo pipefail
cd "$(dirname "$0")/../.."

cargo test -q -p mason-daemon --test bug_alerts 2>&1 | tail -3
cargo test -q -p mason-daemon --test panics a_panic_in_a_background_loop 2>&1 | tail -5
echo "s106 OK"
