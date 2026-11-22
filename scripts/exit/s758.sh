#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s758 exit test: the home view lists every remote run (#767).
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
cargo test -q -p mason-daemon --test it -- remote_runs::
echo "s758 passed"
