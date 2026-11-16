#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s465 exit test: one wait is one finding.
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test -q -p mason-core health
cargo test -q -p mason-core one_wait_is_one_finding
echo "s465: ok"
