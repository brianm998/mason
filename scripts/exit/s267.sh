#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s267 exit test: the Windows task generator and the Windows CPU arithmetic, tested on any platform.
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test -q -p mason-cli task_
cargo test -q -p mason-daemon windows_
echo "s267: ok"
