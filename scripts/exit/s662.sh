#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s662 exit test: the Compose monitor's task numbers are links, and the source scan passes.
set -euo pipefail
cd "$(dirname "$0")/../../clients/compose"
gradle --no-daemon -q :app:test --tests 'mason.monitor.ui.TaskNumberLinksTest' --tests 'mason.monitor.ui.LinkedTextTest'
echo "s662: scan test and link tests passed (allowlist: debt, finding, issue, PR, batch ids only)"
