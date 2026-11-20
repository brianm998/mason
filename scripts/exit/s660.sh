#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s660 exit test: the socket-serving adapter tests are gated to Unix.
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test -q -p mason-adapters --test codex --test conformance --test mcp
grep -q '^#!\[cfg(unix)\]' crates/adapters/tests/codex.rs crates/adapters/tests/conformance.rs crates/adapters/tests/mcp.rs
echo "s660 OK"
