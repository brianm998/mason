#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1220 exit test: a pool wait is not stuck:task (decision 1210).
set -euo pipefail
cd "$(dirname "$0")/../.."

grep -q '^Summary: ' docs/stages/s1220.md
cargo test -q -p mason-core health
cargo test -q -p mason-daemon remote_runs
echo "OK s1220"
