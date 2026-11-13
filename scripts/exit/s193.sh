#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s193 exit test: the whole workspace, tests and examples included, type-checks for
# Windows, and nix stays out of the Windows build. No Windows toolchain is needed: this
# is a check, not a build, so nothing is linked (CI's `windows` job does the real build).
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
target=x86_64-pc-windows-msvc
rustup target list --installed | grep -qx "$target" || rustup target add "$target"

cargo check -q --workspace --all-targets --target "$target"

# nix is Unix-only: it must not appear in the Windows dependency tree.
if cargo tree --workspace --target "$target" -e normal,build,dev --prefix none 2>/dev/null | grep -q '^nix '; then
  echo "FAILED: nix is in the Windows dependency tree" >&2
  exit 1
fi
# The platform crate's Unsupported errors and helpers are tested on every system.
cargo test -q -p mason-platform

echo "s193 ok"
