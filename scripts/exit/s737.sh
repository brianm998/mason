#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Stage 737 exit test: the three remote::tests that were wrong now pass.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
if [ -n "${MASON_SKIP_SWIFT:-}" ] || [ "$(uname)" != Darwin ] || ! command -v swift >/dev/null; then
  echo "s737 skipped: needs macOS and Swift (and is not run by the Rust job)"; exit 0
fi
cargo test -q -p mason-daemon --lib remote::tests::iso_times_and_ref_names
cargo test -q -p mason-daemon --lib remote::tests::a_skip_while_queued_or_running
cargo test -q -p mason-pipeline --bin mason remote::tests::a_swiftpm_project
echo "s737 ok"
