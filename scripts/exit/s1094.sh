#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1094 exit test: a step waiting on a live GitHub run is not silent (decision 1083).
set -euo pipefail
cd "$(dirname "$0")/../.."

grep -q '^Summary: ' docs/stages/s1094.md
cargo test -q -p mason-daemon --lib remote_runs::tests
echo "OK s1094"
