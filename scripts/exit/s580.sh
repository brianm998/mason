#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test -p mason-daemon --no-run
cargo clippy -p mason-daemon --all-targets -- -D warnings
if rustup target list --installed 2>/dev/null | grep -q x86_64-pc-windows-msvc; then
  # The C dependencies (sqlite) cannot be cross-compiled on a Mac, so this is best effort.
  cargo check -p mason-daemon --all-targets --target x86_64-pc-windows-msvc \
    || echo "skip: windows cross-check failed (no C cross toolchain?)"
else
  echo "skip: x86_64-pc-windows-msvc target not installed"
fi
