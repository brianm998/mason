#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s398 exit test: of two tasks on one file, the second starts only after the first lands.
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test -q -p mason-daemon --lib -- file_hold::tests
echo "s398 OK"
