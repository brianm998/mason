#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s401 exit test: a fake agent that commits steadily past a short budget is not stopped at it, only at the cap.
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test -q -p mason-daemon --lib -- deadline::tests
cargo test -q -p mason-daemon --test it deadline_extension::
echo "s401 OK"
