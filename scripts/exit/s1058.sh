#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1058 exit test (multi-machine nodes, phase 4): the views. `GET /nodes` and the owner routes give
# what the monitor shows, a run on a node is a row of "Remote runs", the Swift monitor reads the
# node views (this Mac's switch, a coordinator's nodes), and the docs say how to reach a node daemon
# of another user.
set -euo pipefail
cd "$(dirname "$0")/../.."

grep -q '^Summary: ' docs/stages/s1058.md
grep -q 'Reaching the node' docs/nodes.md
grep -q 'node-accept-toggle' clients/features.toml
cargo test -q -p mason-daemon --test nodes -- remote_runs
# The Swift tests build the whole monitor, which Linux CI does not do (the clients have jobs of their own).
if [ "$(uname)" = Darwin ] && command -v swift >/dev/null && [ -z "${MASON_SKIP_SWIFT:-}" ]; then
  swift test --package-path clients/swift --filter NodesModelTests
fi
echo "OK s1058"
