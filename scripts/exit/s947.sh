#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s947 exit test: the landing crate still builds with the floor-build fallback.
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test -q -p mason-daemon --lib -- outage_tests
echo "s947 OK"
