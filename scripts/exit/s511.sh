#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s511 exit test: an agent that ends its last three full run_check runs red on the same
# failing tests raises one attention item; a different set raises none; a green run clears it.
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test -q -p mason-daemon --lib circling
cargo test -q -p mason-daemon --test circling
echo "s511: ok"
