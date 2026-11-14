#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# A waiting request is reaped by its pid's death, not by a 10 s poll gap (s198).
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
cargo test -q -p mason-daemon --lib waiter
cargo test -q -p mason-daemon --test lease_pid
