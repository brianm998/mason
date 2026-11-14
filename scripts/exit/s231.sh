#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s231 exit test: the daemon crate builds with the fetch-wait change.
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo check -q -p mason-daemon
grep -q 'fetching the remote' crates/daemon/src/jobs.rs
echo "s231: ok"
