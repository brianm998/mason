#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s326 exit test: a bare swift-build monitor at an older commit is rebuilt in place with phase
# reports, and the new binary appears at the same path only after the build worked.
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test -q -p mason-daemon --lib -- monitor_inplace
echo "s326 OK"
