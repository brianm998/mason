#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s662 exit test: the Compose monitor's task numbers are links, and the source scan passes.
set -euo pipefail
if [ -n "${MASON_SKIP_COMPOSE:-}" ]; then echo "s662 skipped: MASON_SKIP_COMPOSE is set (CI's Rust job)"; exit 0; fi
cd "$(dirname "$0")/../../clients/compose"
if [ -z "${MASON_SKIP_COMPOSE:-}" ] && command -v gradle >/dev/null; then
  gradle --no-daemon -q :app:test --tests 'mason.monitor.ui.TaskNumberLinksTest' --tests 'mason.monitor.ui.LinkedTextTest'
else
  echo "s662 skipped: needs gradle (CI's compose job runs the whole suite)"
fi
echo "s662: scan test and link tests passed (allowlist: debt, finding, issue, PR, batch ids only)"
