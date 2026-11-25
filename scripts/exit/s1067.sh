#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1067 exit test: `mason project rename` sends the daemon's owner identity (decision 1056).
set -euo pipefail
cd "$(dirname "$0")/../.."

grep -q '^Summary: ' docs/stages/s1067.md
cargo test -q -p mason-pipeline owner_identity_follows_the_daemon_default_order
bash scripts/exit/s965.sh
echo "OK s1067"
