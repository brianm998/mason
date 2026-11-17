#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s473 exit test: two commits ten minutes apart give one update; urgent goes at once;
# the build asks the broker for compile.
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test -q -p mason-daemon --lib -- update::tests::two_commits update::tests::the_gap_ends update::tests::the_build_asks update::tests::min_gap
echo "s473: ok"
