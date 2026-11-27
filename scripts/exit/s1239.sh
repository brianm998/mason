#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1239 exit test: onboard has no ungated std::os::unix use (decision 1229).
set -euo pipefail
cd "$(dirname "$0")/../.."

grep -q '^Summary: ' docs/stages/s1239.md
! grep -rn 'os::unix\|from_mode' crates/onboard/src
cargo test -q -p mason-onboard
echo "OK s1239"
