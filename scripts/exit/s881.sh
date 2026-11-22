#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s881 exit test: the run_on test follows the placement modes (#896).
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
cargo test -p mason-definition layers
echo "s881 passed"
