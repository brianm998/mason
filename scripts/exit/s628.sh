#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Stage 628 exit test: the adapter harnesses no longer hardcode /tmp.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
if grep -n 'tempdir_in("/tmp")' crates/adapters/tests/codex.rs crates/adapters/tests/conformance.rs; then
    echo "harness still calls tempdir_in(\"/tmp\") directly" >&2
    exit 1
fi
cargo test -p mason-adapters --test codex --test conformance
