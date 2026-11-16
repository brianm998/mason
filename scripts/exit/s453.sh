#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s453 exit test: the settings API routes `failed` and refuses the monitor-only `paused`; that is s430.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
exec bash scripts/exit/s430.sh
