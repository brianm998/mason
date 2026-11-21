#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s735 exit test: the old [remote] name is gone from code that reads project files.
set -euo pipefail
cd "$(dirname "$0")/../.."
grep -q 'table("remote_runs")' crates/daemon/src/config.rs
grep -q 'would clash with the git remote key' crates/daemon/src/config.rs
