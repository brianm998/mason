#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s710 exit test: the landing skip tests still pass with skips checked before the lease.
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test -q -p mason-daemon --test landing_skip
