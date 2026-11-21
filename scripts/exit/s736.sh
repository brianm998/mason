#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s736 exit test: cargo-workspace has shards = 8 and a shard_command calling scripts/test-shard.py.
set -euo pipefail
cd "$(dirname "$0")/../.."
grep -q '^shards = 8$' .mason/mason.toml
grep -q "^shard_command = .*scripts/test-shard.py" .mason/mason.toml
test -f scripts/test-shard.py
echo ok
