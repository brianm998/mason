#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1160 exit test: a node without origin/<base> is a node failure, and `mason node sync` restores
# it (decision 1149).
set -euo pipefail
cd "$(dirname "$0")/../.."

grep -q '^Summary: ' docs/stages/s1160.md
cargo test -q -p mason-daemon --lib node_exec::tests
cargo test -q -p mason-daemon --lib a_base_the_mirror_cannot_set_is_reported
cargo test -q -p mason-daemon --test nodes a_node_without_origin_base_is_a_node_failure
echo "OK s1160"
