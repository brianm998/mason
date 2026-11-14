#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s287 exit test: a review prompt carries the diff.
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test -q -p mason-daemon --lib -- review_diff_tests
echo "s287 OK"
