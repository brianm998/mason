#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1222 exit test: run_check bounds the manual merge rounds (decision 1212).
set -euo pipefail
cd "$(dirname "$0")/../.."

grep -q '^Summary: ' docs/stages/s1222.md
cargo test -q -p mason-daemon basefold
echo "OK s1222"
