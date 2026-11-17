#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s397 exit test: a retry diagnosis is applied by itself for any project when the failure was not the code.
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test -q -p mason-daemon --lib -- diagnose::tests
cargo test -q -p mason-daemon --test it diagnose_retry::
echo "s397 OK"
