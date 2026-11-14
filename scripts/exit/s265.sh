#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s265 exit test: Unix keeps exec; the platform and daemon update tests pass.
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test -q -p mason-platform -- wait_until replacing_a_missing_program
cargo test -q -p mason-daemon --lib -- update::
echo "s265 OK"
