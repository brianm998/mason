#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1307 exit test: the projsettings tests pass, including the two that failed on the base
# (decision 1297).
set -euo pipefail
cd "$(dirname "$0")/../.."

grep -q '^Summary: ' docs/stages/s1307.md
cargo test -q -p mason-daemon --test it projsettings::
echo "OK s1307"
