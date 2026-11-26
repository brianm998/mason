#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1159 exit test: a coordinator with an over-long name can be paired and removed (decision 1148).
set -euo pipefail
cd "$(dirname "$0")/../.."

grep -q '^Summary: ' docs/stages/s1159.md
cargo test -q -p mason-daemon --lib coordinator_name
cargo test -q -p mason-daemon --test nodes
echo "OK s1159"
