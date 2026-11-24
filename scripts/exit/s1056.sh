#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1056 exit test (multi-machine nodes, phase 2): two daemons on loopback. A run goes to the node
# and comes back as a local run would (verdict, output, results file, `ran_on`), a red suite is
# the same red, a batch's suite run on the node lands the batch on the coordinator, and the node
# keeps no push configuration.
set -euo pipefail
cd "$(dirname "$0")/../.."

grep -q '^Summary: ' docs/stages/s1056.md
grep -q 'environment audit' docs/decisions/1047-multi-machine-nodes.md
cargo test -q -p mason-daemon --lib -- node_run:: node_exec:: node_serve:: nodes:: place::
cargo test -q -p mason-daemon --test nodes
echo "OK s1056"
