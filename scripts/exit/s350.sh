#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s350 exit test: the daemon-down announcement says "mason D", not "masond".
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test -q -p mason-core daemon_down_is_spoken_as_mason_d
echo "s350 OK"
