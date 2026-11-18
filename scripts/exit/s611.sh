#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Stage 611 exit test: every script carries its SPDX header, including s580.sh.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
bash scripts/check-spdx.sh
