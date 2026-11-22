#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s757 exit test: runner waiters are ranked and other workflows' jobs count against the pool (#766).
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
cargo test -q -p mason-daemon --lib -- a_landing_lint_gets_the_next_runner_unit other_workflows
echo "s757 passed"
