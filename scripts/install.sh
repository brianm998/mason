#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Installs MASON (mason-pipeline) from a release: downloads the archive for this machine,
# checks its SHA-256 against the release's SHA256SUMS, and only then copies the binaries.
#
#   curl -fsSL https://<release host>/install.sh | bash
#   bash install.sh [--version X.Y.Z] [--prefix DIR] [--base-url URL]
#
# --base-url (or MASON_RELEASE_URL) is a folder holding mason-<version>-<target>.tar.gz and
# SHA256SUMS; https://, http:// and file:// all work, which is how the test runs against a local
# fake release. A checksum that does not match stops the install before anything is copied.
#
# Signatures: the release publishes SHA256SUMS.minisig, made with the owner's minisign key. Once
# that key is pinned below (PINNED_PUBKEY), or given as MASON_MINISIGN_PUBKEY, the install is
# REFUSED unless minisign is installed and the signature verifies; an unsigned release, a bad
# signature and a missing minisign all stop it. While the placeholder is still pinned and no key
# is given, it says plainly that the signature was NOT verified. The placeholder is never used as
# a key. The checksum alone proves a download matches the checksum file, not that it is the owner's.
#
# It never uses sudo and never edits your shell profile; it says what to add to PATH.
set -euo pipefail

version="${MASON_VERSION:-latest}"
prefix="${MASON_PREFIX:-$HOME/.local}"
base_url="${MASON_RELEASE_URL:-https://github.com/brianm998/mason/releases}"
# The owner's minisign public key (the second line of minisign.pub; see docs/RELEASING.md).
# TRUST: change only with the owner's approval. Until they pin it, this placeholder is not a key.
PUBKEY_PLACEHOLDER="REPLACE_WITH_MINISIGN_PUBLIC_KEY"
PINNED_PUBKEY="$PUBKEY_PLACEHOLDER"
BINS="mason masond step-host mason-mcp"
IDENTITY="mason: command-line client for the MASON pipeline"

die() { echo "install: $*" >&2; exit 1; }
say() { echo "install: $*"; }

while [ $# -gt 0 ]; do
  case "$1" in
    --version) [ $# -ge 2 ] || die "--version needs a value"; version=$2; shift 2 ;;
    --prefix) [ $# -ge 2 ] || die "--prefix needs a value"; prefix=$2; shift 2 ;;
    --base-url) [ $# -ge 2 ] || die "--base-url needs a value"; base_url=$2; shift 2 ;;
    -h|--help) sed -n '3,20p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) die "unknown option $1 (try --help)" ;;
  esac
done

target() {
  local os arch
  os=$(uname -s); arch=$(uname -m)
  case "$arch" in arm64|aarch64) arch=aarch64 ;; x86_64|amd64) arch=x86_64 ;; *) die "no build for CPU $arch" ;; esac
  case "$os" in
    Darwin) echo "$arch-apple-darwin" ;;
    Linux) echo "$arch-unknown-linux-gnu" ;;
    *) die "no build for $os; try: cargo install mason-pipeline" ;;
  esac
}

sha256_of() {
  if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | cut -d' ' -f1
  elif command -v shasum >/dev/null 2>&1; then shasum -a 256 "$1" | cut -d' ' -f1
  else die "need sha256sum or shasum to verify the download"; fi
}

fetch() {  # URL DEST
  case "$1" in
    file://*) cp "${1#file://}" "$2" ;;
    *) command -v curl >/dev/null 2>&1 || die "need curl"; curl -fsSL "$1" -o "$2" ;;
  esac
}

# Is the `mason` that would run first this program? Asked of the binary, not guessed from its name.
identity_of() { local out; if out=$("$1" --help 2>/dev/null); then printf '%s\n' "$out" | sed -n 1p; else echo "unknown"; fi; }

report_clash() {  # DEST_BIN_DIR
  local dest=$1 first found
  if found=$(command -v mason 2>/dev/null); then :; else found=""; fi
  if [ -z "$found" ]; then
    say "\`mason\` is not on your PATH yet. Add this to your shell profile:  export PATH=\"$dest:\$PATH\""
    return 0
  fi
  if [ "$(cd "$(dirname "$found")" && pwd -P)/$(basename "$found")" = "$(cd "$dest" && pwd -P)/mason" ]; then
    say "\`mason\` on your PATH is the one just installed"
    return 0
  fi
  first=$(identity_of "$found")
  case "$first" in
    "$IDENTITY"*) say "another copy of mason-pipeline is first on your PATH: $found. Put $dest before it, or remove that copy" ;;
    *)
      say "WARNING: a different \`mason\` is first on your PATH: $found${first:+ (it says: \"$first\")}"
      say "  Typing \`mason\` runs that one, not MASON. To resolve it, either:"
      say "    - put $dest before it:  export PATH=\"$dest:\$PATH\""
      say "    - or call MASON by its full path: $dest/mason (an alias such as  alias mason-pipeline=$dest/mason  works too)"
      say "    - for Dart's mason_cli:  dart pub global deactivate mason_cli"
      ;;
  esac
}

t=$(target)
tmp=$(mktemp -d "${TMPDIR:-/tmp}/mason-install.XXXXXX")
trap 'rm -rf "$tmp"' EXIT

if [ "$version" = latest ]; then
  case "$base_url" in
    file://*) die "with a file:// release, give --version" ;;
    *) release_dir="$base_url/latest/download" ;;
  esac
  archive="mason-$t.tar.gz"
else
  case "$base_url" in
    file://*) release_dir="$base_url" ;;
    *) release_dir="$base_url/download/v$version" ;;
  esac
  archive="mason-$version-$t.tar.gz"
fi

say "downloading $archive"
fetch "$release_dir/$archive" "$tmp/$archive" || die "could not download $release_dir/$archive"
fetch "$release_dir/SHA256SUMS" "$tmp/SHA256SUMS" || die "could not download $release_dir/SHA256SUMS; refusing to install what cannot be checked"

# The signature comes first: nothing in SHA256SUMS is believed until it verifies.
key="${MASON_MINISIGN_PUBKEY:-$PINNED_PUBKEY}"
if [ "$key" = "$PUBKEY_PLACEHOLDER" ] && [ -z "${MASON_MINISIGN_PUBKEY:-}" ]; then
  say "signature NOT verified: no signing key is pinned yet. The checksum below only matches the release's own checksum file"
else
  case "$key" in
    RW*) [ "${#key}" -eq 56 ] || die "the minisign public key is malformed (not 56 characters); refusing to install" ;;
    *) die "the minisign public key is not a key ($key); refusing to install" ;;
  esac
  command -v minisign >/dev/null 2>&1 || die "a signing key is pinned, so minisign is needed to check the release; install it (https://jedisct1.github.io/minisign/) and retry. Nothing was installed"
  fetch "$release_dir/SHA256SUMS.minisig" "$tmp/SHA256SUMS.minisig" 2>/dev/null \
    || die "release has no SHA256SUMS.minisig; refusing an unsigned release. Nothing was installed"
  minisign -Vm "$tmp/SHA256SUMS" -P "$key" -x "$tmp/SHA256SUMS.minisig" >/dev/null 2>&1 \
    || die "SIGNATURE CHECK FAILED for SHA256SUMS. Nothing was installed"
  say "signature ok (minisign)"
fi

want=$(awk -v f="$archive" '$2 == f || $2 == "*" f { print $1 }' "$tmp/SHA256SUMS" | head -n 1)
[ -n "$want" ] || die "SHA256SUMS has no entry for $archive; refusing to install"
got=$(sha256_of "$tmp/$archive")
if [ "$want" != "$got" ]; then
  die "CHECKSUM MISMATCH for $archive (expected $want, got $got). Nothing was installed. The download is corrupt or has been tampered with"
fi
say "checksum ok ($got)"

mkdir "$tmp/x"
tar -xzf "$tmp/$archive" -C "$tmp/x" || die "could not unpack $archive"
dest="$prefix/bin"
mkdir -p "$dest"
installed=""
for b in $BINS; do
  f=$(find "$tmp/x" -type f -name "$b" | head -n 1)
  [ -n "$f" ] || { [ "$b" = mason ] && die "the archive has no \`mason\` binary"; continue; }
  cp "$f" "$dest/$b.new" && chmod 755 "$dest/$b.new" && mv "$dest/$b.new" "$dest/$b"
  installed="$installed $b"
done
say "installed:$installed in $dest"
report_clash "$dest"
say "next: cd your-project && mason init   (and \`mason doctor\` rechecks the setup any time)"
