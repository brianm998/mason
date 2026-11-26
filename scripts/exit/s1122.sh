#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1122 exit test: "Update masond" updates every node too (decision 1111).
set -euo pipefail
cd "$(dirname "$0")/../.."

grep -q '^Summary: ' docs/stages/s1122.md
# A coordinator and a fake node at different runtime SHAs and different node_proto: the update path
# sends the node the commit (building, sent, installed), the node list and `mason update status`
# show it, a node on the coordinator's commit is up to date, and `node update --all` sends at once.
cargo test -q -p mason-daemon --test nodes fleet::
# needs_runtime across protocols, the state words, and the route and status keys the contract names.
cargo test -q -p mason-daemon --lib a_node_needs_the_runtime
cargo test -q -p mason-daemon --lib a_node_on_an_older_protocol_that_lacks
cargo test -q -p mason-daemon --lib node_fleet::
echo "OK s1122"
