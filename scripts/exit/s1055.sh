#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1055 exit test (multi-machine nodes, phase 1): two daemons on loopback pair by code, the
# coordinator sees the node's switch and sends it settings, an unpaired certificate gets 403, and
# the node keeps no token and no copy of the settings.
set -euo pipefail
cd "$(dirname "$0")/../.."

grep -q '^Summary: ' docs/stages/s1055.md
test -s docs/nodes.md
grep -q 'mason node add' docs/nodes.md
cargo test -q -p mason-daemon --lib -- config:: node_serve:: nodes:: thermal::
cargo test -q -p mason-store --test nodes
# The exit test proper: two daemons, separate data and configuration directories.
cargo test -q -p mason-daemon --test nodes
echo "OK s1055"
