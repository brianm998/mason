#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s935 exit test: a src/ change narrows the crate's unit tests to the module and its referrers.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
cargo test -q -p mason-daemon --lib -- affected::tests::
echo "s935 passed"
