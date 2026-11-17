#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s485 exit test: the APNs token cache is keyed by the key, so the daemon's apns tests
# do not depend on order. Runs the binary N times (default 5).
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test -q -p mason-adapter-api apns
for i in $(seq 1 "${S485_RUNS:-5}"); do
  cargo test -q -p mason-daemon --test apns
  echo "run $i: green"
done
