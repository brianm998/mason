#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Shared by tarball.sh and deb.sh (sourced, not run). The asset names and layout are the
# contract in docs/RELEASING.md.
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
# fake-claude is the scripted agent the new build's self-test (`masond selftest`) runs, which
# looks for it next to masond; `mason update` from a release cannot pass without it.
BINS="mason masond step-host mason-mcp fake-claude"

pkg_die() { echo "package: $*" >&2; exit 1; }

# The workspace version, the same line scripts/release-version.sh reads.
pkg_version() {
  local v
  v=$(awk '/^\[workspace\.package\]/{f=1;next} /^\[/{f=0} f && /^version *=/{gsub(/[" ]/,"",$0); sub(/^version=/,"",$0); print; exit}' "$ROOT/Cargo.toml")
  [ -n "$v" ] || pkg_die "no workspace version in Cargo.toml"
  echo "$v"
}

# Build the four binaries for TARGET, strip them and put them in DEST. On macOS every copied
# Mach-O is signed afterwards (stripping invalidates the signature): ad hoc unless
# MASON_CODESIGN_IDENTITY names a real identity. MASON_PACKAGE_NO_BUILD=1 reuses an earlier build.
pkg_build() {  # TARGET DEST
  local target=$1 dest=$2 tdir b
  tdir="${CARGO_TARGET_DIR:-$ROOT/target}/$target/release"
  if [ "${MASON_PACKAGE_NO_BUILD:-0}" != 1 ]; then
    (cd "$ROOT" && cargo build --release --target "$target" \
      --bin mason --bin masond --bin step-host --bin mason-mcp --bin fake-claude) || pkg_die "cargo build failed for $target"
  fi
  mkdir -p "$dest"
  for b in $BINS; do
    [ -f "$tdir/$b" ] || pkg_die "missing $tdir/$b"
    cp "$tdir/$b" "$dest/$b"
    chmod 755 "$dest/$b"
    if ! "${STRIP:-strip}" "$dest/$b" 2>/dev/null; then
      echo "package: warning: could not strip $b (cross-target strip?); shipping it unstripped" >&2
    fi
    if [ "$(uname -s)" = Darwin ] && file "$dest/$b" | grep -q Mach-O; then
      codesign --force -s "${MASON_CODESIGN_IDENTITY:--}" "$dest/$b" || pkg_die "codesign failed for $b"
    fi
  done
  cp "$ROOT/packaging/mason-askpass.sh" "$dest/mason-askpass.sh"
  chmod 755 "$dest/mason-askpass.sh"
}
