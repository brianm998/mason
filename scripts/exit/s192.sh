#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s192 exit test: `mason service install --print` names the absolute masond path and --managed,
# and the unit tests pass. Never touches a real launchd agent or systemd unit.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
cargo build -q -p mason-pipeline
out=$(target/debug/mason service install --print)
echo "$out"
masond="$(cd target/debug && pwd)/masond"
echo "$out" | grep -qF -- "$masond"
echo "$out" | grep -qF -- "--managed"
cargo test -q -p mason-pipeline service::
echo "s192: ok"
