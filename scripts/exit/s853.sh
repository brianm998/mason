#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s853 exit test: the abandoned-run tests pass.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
cargo test -q -p mason-daemon --test it abandoned_run
echo "s853 passed"
