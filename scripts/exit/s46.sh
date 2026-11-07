#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Tile labels (s46) exit test: the daemon sends a label, needs_attention reads "stuck".
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
cargo test -q -p mason-daemon tile_label
cargo test -q -p mason-daemon --test protocol
cargo test -q -p mason-conformance
echo "s46: ok"
