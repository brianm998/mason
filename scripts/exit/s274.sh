#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s274 exit test: install.sh with a pinned test key accepts a signed SHA256SUMS and refuses a
# tampered, unsigned or wrongly signed one, and never trusts the placeholder as a key. The key is
# generated here and thrown away. Skipped with a message when minisign is not installed.
set -euo pipefail
cd "$(dirname "$0")/../.."

# The placeholder stays a placeholder in the committed script, and a real key replaces it whole.
grep -q '^PINNED_PUBKEY="\$PUBKEY_PLACEHOLDER"$' scripts/install.sh \
  || grep -Eq '^PINNED_PUBKEY="RW[A-Za-z0-9+/=]{54}"$' scripts/install.sh \
  || { echo "FAILED: PINNED_PUBKEY is neither the placeholder nor a minisign key" >&2; exit 1; }

if ! command -v minisign >/dev/null 2>&1; then
  echo "s274: minisign is not installed, signature tests skipped"; exit 0
fi

arch=$(uname -m); case "$arch" in arm64|aarch64) arch=aarch64 ;; *) arch=x86_64 ;; esac
case "$(uname -s)" in
  Darwin) target="$arch-apple-darwin" ;;
  Linux) target="$arch-unknown-linux-gnu" ;;
  *) echo "s274: no tarball target for $(uname -s), skipped"; exit 0 ;;
esac

tmp=$(mktemp -d "${TMPDIR:-/tmp}/s274.XXXXXX")
trap 'rm -rf "$tmp"' EXIT
ver=9.9.9; name="mason-$ver-$target"
mkdir -p "$tmp/stage/$name" "$tmp/rel" "$tmp/keys" "$tmp/other"
printf '#!/bin/sh\necho "mason: command-line client for the MASON pipeline"\n' > "$tmp/stage/$name/mason"
chmod 755 "$tmp/stage/$name/mason"
tar -czf "$tmp/rel/$name.tar.gz" -C "$tmp/stage" "$name"
bash scripts/package/checksums.sh "$tmp/rel" >/dev/null
for d in keys other; do
  minisign -G -W -f -s "$tmp/$d/k.key" -p "$tmp/$d/k.pub" >/dev/null
done
pub=$(sed -n 2p "$tmp/keys/k.pub")
cp "$tmp/rel/SHA256SUMS" "$tmp/good-sums"

install() {  # extra env as VAR=val args; returns install.sh's status, output in $tmp/out
  rm -rf "$tmp/prefix"
  env "$@" bash scripts/install.sh --version "$ver" --base-url "file://$tmp/rel" --prefix "$tmp/prefix" >"$tmp/out" 2>&1
}
expect_refused() {  # label, text it must say
  if install MASON_MINISIGN_PUBKEY="$pub"; then echo "FAILED: $1 was installed" >&2; cat "$tmp/out" >&2; exit 1; fi
  grep -q "$2" "$tmp/out" || { echo "FAILED: $1: wrong message" >&2; cat "$tmp/out" >&2; exit 1; }
  [ ! -e "$tmp/prefix/bin/mason" ] || { echo "FAILED: $1 left a binary" >&2; exit 1; }
  echo "ok: $1 refused"
}

# unsigned
expect_refused "an unsigned release" "no SHA256SUMS.minisig"
# signed by the pinned key
minisign -S -s "$tmp/keys/k.key" -m "$tmp/rel/SHA256SUMS" -x "$tmp/rel/SHA256SUMS.minisig" >/dev/null
install MASON_MINISIGN_PUBKEY="$pub" || { echo "FAILED: a good signature was refused" >&2; cat "$tmp/out" >&2; exit 1; }
grep -q "signature ok" "$tmp/out" || { echo "FAILED: no 'signature ok'" >&2; exit 1; }
[ -x "$tmp/prefix/bin/mason" ] || { echo "FAILED: nothing installed" >&2; exit 1; }
echo "ok: a good signature installs"
# tampered SHA256SUMS (the tarball hash is rewritten to match a tampered tarball)
printf '# tampered\n' >> "$tmp/rel/SHA256SUMS"
expect_refused "a tampered SHA256SUMS" "SIGNATURE CHECK FAILED"
cp "$tmp/good-sums" "$tmp/rel/SHA256SUMS"
# signed by another key
minisign -S -s "$tmp/other/k.key" -m "$tmp/rel/SHA256SUMS" -x "$tmp/rel/SHA256SUMS.minisig" >/dev/null
expect_refused "a release signed by another key" "SIGNATURE CHECK FAILED"
# the placeholder is not a key
if install MASON_MINISIGN_PUBKEY=REPLACE_WITH_MINISIGN_PUBLIC_KEY; then
  echo "FAILED: the placeholder was accepted as a key" >&2; exit 1
fi
grep -q "not a key" "$tmp/out" || { echo "FAILED: placeholder: wrong message" >&2; cat "$tmp/out" >&2; exit 1; }
echo "ok: the placeholder is refused"
echo "s274: ok"
