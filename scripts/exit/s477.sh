#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s477 exit test: the holds, crash and landing_crash targets of mason-daemon pass.
set -euo pipefail
cd "$(dirname "$0")/../.."
for t in holds crash landing_crash; do
  cargo test -q -p mason-daemon --test "$t"
done
echo "s477: holds, crash, landing_crash green"
