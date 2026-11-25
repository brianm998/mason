#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1082 exit test: a planned start is not counted as a restart (decision 1071).
set -euo pipefail
cd "$(dirname "$0")/../.."

grep -q '^Summary: ' docs/stages/s1082.md
cargo test -q -p mason-core a_planned_start_is_not_a_restart
echo "OK s1082"
