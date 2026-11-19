#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# The exit-scripts runner names failing scripts in its last stdout line.
set -euo pipefail
cd "$(dirname "$0")/../.."
bash -n scripts/run-exit-scripts.sh
grep -q 'FAILED scripts:' scripts/run-exit-scripts.sh
grep -q '^exit \$fail$' scripts/run-exit-scripts.sh
echo "s604: all checks passed"
