#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1256 exit test: with a fake GitHub, a test step's suite skipped as it is about to go out, and a
# task landed, leave no run and no mason/run ref behind (decision 1246).
set -euo pipefail
cd "$(dirname "$0")/../.."

grep -q '^Summary: ' docs/stages/s1256.md
cargo test -q -p mason-daemon --lib untracked_run
echo "OK s1256"
