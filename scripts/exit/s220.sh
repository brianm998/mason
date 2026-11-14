#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s220 exit test: MonitorCoreTest passes 5 runs in a row.
set -euo pipefail
cd "$(dirname "$0")/../../clients/compose"
for i in 1 2 3 4 5; do
  echo "run $i"
  ./gradlew :core:test --rerun-tasks --tests 'mason.monitor.MonitorCoreTest'
done
echo "s220: 5 of 5 green"
