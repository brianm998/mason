#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s232 exit test: a signaled CLI is recognized.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
cargo test -q -p mason-daemon --lib signaled_tests
echo "s232 passed"
