#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s318 exit test: bounded() works with no `timeout` binary on PATH.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
bash scripts/tests/portable-timeout.sh
echo "s318 ok"
