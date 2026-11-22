#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s742 exit test: unknown remote subcommands are rejected before the daemon lookup.
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test -p mason-pipeline --test remote_dispatch
cargo fmt --check
cargo clippy -p mason-pipeline -- -D warnings
