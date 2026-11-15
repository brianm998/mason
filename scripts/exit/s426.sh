#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s426 exit test: a skipped check is paid back after its task lands.
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test -q -p mason-daemon --test testing -- skip
echo "s426 OK"
