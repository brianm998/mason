#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s477 exit test: the holds target and the crash and landing_crash modules of mason-daemon pass.
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test -q -p mason-daemon --test holds
cargo test -q -p mason-daemon --test it crash::
echo "s477: holds, crash, landing_crash green"
