#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test -q -p mason-daemon --test world_slots
cargo build -q -p mason-daemon --tests
echo "s638 OK"
