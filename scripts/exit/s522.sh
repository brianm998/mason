#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s522 exit test: a develop agent's tool-call limit covers a run_check's queue wait.
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test -q -p mason-daemon --lib develop_tool_timeout_covers
