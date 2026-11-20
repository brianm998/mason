#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s707 exit test: the input-key reuse module's tests pass, including the two-crate scratch
# workspace where the second task changes only crate B.
set -euo pipefail
cd "$(dirname "$0")/../.."

grep -q '^Summary: ' docs/stages/s707.md || { echo "s707: stage note has no Summary line" >&2; exit 1; }
[ -f docs/decisions/0700-reuse-a-green-cargo-test-target-on-the-same-inputs.md ] || { echo "s707: decision missing" >&2; exit 1; }
cargo test -p mason-daemon --lib reuse:: 2>&1 | tail -20
echo "s707: ok"
