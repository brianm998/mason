#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s906 exit test: the fake host materialises a PR head with git, not tar.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
cargo test -p mason-adapters --test github
echo "s906 passed"
