#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1159 exit test: the node-goes-away test waits for the coordinator to follow the job.
set -euo pipefail
cd "$(dirname "$0")/../.."

grep -q '^Summary: ' docs/stages/s1159.md
cargo test -q -p mason-daemon --test nodes
echo "OK s1159"
