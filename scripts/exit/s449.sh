#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s449 exit test: the state matrix, codec and daemon protocol tests know the Deferred state.
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test -q -p mason-engine --test matrix
cargo test -q -p mason-store --lib
cargo test -q -p mason-daemon --test it protocol::
for s in s02 s03 s50 s73 s211 s325 s337; do
  bash "scripts/exit/$s.sh"
done
echo "s449 OK"
