#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s874 exit test: deliberate holds do not count as landing idle.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
cargo test -q -p mason-daemon --lib -- idle_tests
echo "s874 passed"
