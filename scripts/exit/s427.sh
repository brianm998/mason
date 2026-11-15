#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s427 exit test: the timeout text names waiting when most of the run was queue time.
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test -q -p mason-daemon --lib -- jobs::tests
echo "s427 OK"
