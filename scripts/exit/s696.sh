#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s696 exit test: check-spdx passes on the s653 exit script.
set -euo pipefail
cd "$(dirname "$0")/../.."
bash scripts/check-spdx.sh scripts/exit/s653.sh
echo "s696 OK"
