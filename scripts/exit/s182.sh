#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s182 exit test: the landing idle counter resets when the idle spell ends.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
cargo test -q -p mason-daemon --lib idle_tests
echo "s182: ok"
