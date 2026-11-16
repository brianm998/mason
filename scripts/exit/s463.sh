#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s463 exit test: the load finding names its CPU source.
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test -q -p mason-core load_finding
cargo test -q -p mason-daemon --lib load_tests
echo "s463: ok"
