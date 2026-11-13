#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s233 exit test: a loss shared with other tasks' hosts does not use up restarts.
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test -q -p mason-adapters run_of_tests
