#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s859 exit test: the fake host's checks run through the project shell (#828).
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
cargo test -p mason-adapters --test github
echo "s859 passed"
