#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s513 exit test: a Rust executable built here carries a signature (Intel Macs need the
# linker flag from .cargo/config.toml); elsewhere only the config and monitor build are checked.
set -euo pipefail
cd "$(dirname "$0")/../.."
grep -q 'adhoc_codesign' .cargo/config.toml
grep -q 'SIGN_APP=1' crates/daemon/src/update.rs
if [ "$(uname -s)" = Darwin ] && [ "$(uname -m)" = x86_64 ]; then
  d=$(mktemp -d /tmp/s513.XXXXXX)
  trap 'rm -rf "$d"' EXIT
  mkdir -p "$d/src" "$d/.cargo"
  printf '[package]\nname="s513"\nversion="0.0.0"\nedition="2021"\n[workspace]\n' >"$d/Cargo.toml"
  echo 'fn main(){}' >"$d/src/main.rs"
  cp .cargo/config.toml "$d/.cargo/config.toml"
  (cd "$d" && cargo build -q --target x86_64-apple-darwin)
  codesign -v "$d/target/x86_64-apple-darwin/debug/s513"
fi
echo "s513 OK"
