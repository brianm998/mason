#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s841 exit test: core stays pure with fixtures embedded, and the gate still rejects IO.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
tmp=$(mktemp -d); trap 'rm -rf "$tmp"' EXIT
bash scripts/check-pure.sh crates/core/src crates/engine/src crates/scheduler/src
mkdir -p "$tmp/src"
echo 'fn f() { let _ = std::time::SystemTime::now(); }' > "$tmp/src/x.rs"
if bash scripts/check-pure.sh "$tmp/src" >/dev/null 2>&1; then
  echo "check-pure.sh accepted a planted violation" >&2; exit 1
fi
cargo test -p mason-core
echo "s841 OK"
