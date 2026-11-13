#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s209 exit test: the tree is fmt clean, and ci.yml cancels superseded runs and keeps the
# macOS legs of exit-scripts and monitor-compose off the push run.
set -uo pipefail
cd "$(dirname "$0")/../.."
cargo fmt --all --check || { echo "s209: not fmt clean"; exit 1; }
cargo test -q -p mason-conformance --test ci_gates || exit 1
echo "s209: ok"
