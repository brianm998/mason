#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s478 exit test: in a scratch git repo a README-only commit after a stored baseline reuses it,
# and a change under crates/, to the suite's definition or under .mason/ does not.
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test -q -p mason-daemon --lib reuse_tests
echo "s478: baseline reuse verified in a scratch repo"
