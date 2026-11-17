#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s507 exit test: the engine matrix test lists each event once and passes.
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test -q -p mason-engine --test matrix
echo "s507 OK"
