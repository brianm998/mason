#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s655 exit test: every tracked file carries its SPDX header.
set -euo pipefail
cd "$(dirname "$0")/../.."
bash scripts/check-spdx.sh
echo "s655 OK"
