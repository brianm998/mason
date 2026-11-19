#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Stage 594 exit test: launch-frozen reads only this daemon's launches.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
if grep -n 'launcher::launching()' crates/daemon/src/attention.rs crates/daemon/src/health.rs; then
  echo "daemon reads the process-wide launcher list"; exit 1
fi
cargo test -q -p mason-step-host --test host -- launch
cargo test -q -p mason-daemon launch_frozen
