#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1229 exit test: net_outage tests are gated to Unix and still pass here (decision 1219).
set -euo pipefail
cd "$(dirname "$0")/../.."
head -n 3 crates/daemon/tests/it/net_outage.rs | grep -q 'cfg(unix)'
cargo test -q -p mason-daemon --test it net_outage::
