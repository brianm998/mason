#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1101 exit test: a finished batch has left no integration branch.
set -euo pipefail
cd "$(dirname "$0")/../.."

grep -q '^Summary: ' docs/stages/s1101.md
cargo test -q -p mason-daemon --test it landing
echo "OK s1101"
