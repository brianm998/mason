#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s730 exit test: with a fake `cargo` as the real tool, the shim directory that goes first on a
# remote task's agent PATH refuses `cargo test -p x` with exit 69 and passes `cargo check -p x`
# through (the fake records it). Runs the daemon's agentshim tests, which build exactly that
# setup (fake tools, shims written by `agentshim::write`, PATH made by `agentshim::apply`).
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
cargo test -q -p mason-daemon --lib agentshim:: 2>&1 | tee /dev/stderr | grep -q "test result: ok"
echo "s730 ok"
