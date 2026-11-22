#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s902 exit test: a check's wait is credited every second (#813).
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
cargo test -p mason-daemon a_check_wait_is_credited
bash scripts/exit/s787.sh
