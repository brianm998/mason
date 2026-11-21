#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s751 exit test: a narrowed run keeps the template's cargo flags before the module filter (#764).
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
cargo test -q -p mason-daemon --lib -- scope::tests::expand_
echo "s751 passed"
