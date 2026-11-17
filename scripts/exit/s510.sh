#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s510 exit test: a dropped-line send-back carries ready-to-paste removes entries for replaced lines.
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test -q -p mason-landing --lib -- audit::tests::a_replaced_dropped_line_yields_a_removes_block_that_parses
echo "s510 OK"
