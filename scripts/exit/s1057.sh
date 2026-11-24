#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1057 exit test (multi-machine nodes, phase 3): each row of the failure table on two loopback
# daemons. Drain (the default) lets a running job finish on the node and takes no new one; cancel
# (`on_switch_off = "cancel"` or `mason node stop-jobs`) ends the job and the run finishes here,
# green, with the node's job directory cleaned up; a node that goes away is given up on and its
# orphan killed when its lease runs out; a restarted coordinator takes up its job; a node that
# restarts keeps a finished job; a node on another protocol is never used. No duration is asserted.
set -euo pipefail
cd "$(dirname "$0")/../.."

grep -q '^Summary: ' docs/stages/s1057.md
cargo test -q -p mason-daemon --lib -- node_run:: node_exec:: nodes:: place::
cargo test -q -p mason-daemon --test nodes
echo "OK s1057"
