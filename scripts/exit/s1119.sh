#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1119 exit test: a node whose data directory is as long as the documented one runs the job
# `t1113r6691-check-cargo-workspace-build3` through the node runner, past host start to its end,
# and the node doctor's socket-length check passes there. A socket path of 101 characters is
# refused by length before a host starts, and a host that exits early says why. No duration is
# asserted.
set -euo pipefail
cd "$(dirname "$0")/../.."

grep -q '^Summary: ' docs/stages/s1119.md
cargo test -q -p mason-step-host --test host -- exits_before_it_is_up over_the_limit
cargo test -q -p mason-daemon --lib -- node_run:: node_doctor::
cargo test -q -p mason-daemon --test nodes -- documented_length
cargo test -q -p mason-cli --bin mason -- doctor::
echo "OK s1119"
