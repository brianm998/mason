#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s598 exit test: s13 no longer masks Gradle's status, and the Compose tests do not poll on a clock.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
if grep -n 'gradle.*head -5' scripts/exit/s13.sh; then echo "s13 still masks gradle" >&2; exit 1; fi
if grep -n 'Thread.sleep' clients/compose/core/src/test/kotlin/mason/monitor/MonitorCoreTest.kt clients/compose/core/src/test/kotlin/mason/monitor/RefusedLinkTest.kt; then
  echo "a Compose test still sleeps to poll" >&2; exit 1
fi
echo "s598 OK"
