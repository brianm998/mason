#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s286 exit test: a results_file pattern reads every matching file.
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test -q -p mason-daemon --lib -- results_file_tests
echo "s286 OK"
