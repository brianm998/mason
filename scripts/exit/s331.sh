#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s331 exit test: install.sh can add a `mason()` shell function to the login shell's profile.
# In a temp HOME, against a local fake release (no network):
#  - --profile writes exactly one marked block; a second run replaces it, not duplicates it
#  - "n", --no-profile and a non-interactive run leave every file byte-identical
#  - a fresh bash's `mason --version` runs the runtime named in runtimes/current, with its arguments
#  - without runtimes/current the function falls back to ~/.config/mason/bin/mason
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
root=$(mktemp -d /tmp/mason-s331.XXXXXX)
trap 'rm -rf "$root"' EXIT
fail() { echo "FAIL: $*" >&2; exit 1; }

rel="$root/release"; mkdir -p "$rel/pkg"
for b in mason masond step-host mason-mcp; do
  printf '#!/bin/sh\necho "mason: command-line client for the MASON pipeline (fake %s)"\n' "$b" > "$rel/pkg/$b"; chmod +x "$rel/pkg/$b"
done
case "$(uname -s)-$(uname -m)" in
  Darwin-arm64) tgt=aarch64-apple-darwin ;; Darwin-x86_64) tgt=x86_64-apple-darwin ;;
  Linux-aarch64|Linux-arm64) tgt=aarch64-unknown-linux-gnu ;; Linux-x86_64) tgt=x86_64-unknown-linux-gnu ;;
  *) fail "no target for this machine" ;;
esac
tar -czf "$rel/mason-9.9.9-$tgt.tar.gz" -C "$rel/pkg" .
sha=$( (sha256sum "$rel/mason-9.9.9-$tgt.tar.gz" 2>/dev/null || shasum -a 256 "$rel/mason-9.9.9-$tgt.tar.gz") | cut -d' ' -f1)
echo "$sha  mason-9.9.9-$tgt.tar.gz" > "$rel/SHA256SUMS"

home="$root/home"; mkdir -p "$home"
unset XDG_DATA_HOME
inst() { env HOME="$home" SHELL=/bin/bash bash scripts/install.sh --version 9.9.9 --base-url "file://$rel" --prefix "$home/.local" "$@"; }
snap() { (cd "$home" && find . -type f ! -path './.local/*' -exec cksum {} + | sort); }
printf 'export FOO=1\n' > "$home/.bash_profile"; printf 'export BAR=2\n' > "$home/.profile"

echo "== declined, --no-profile, non-interactive: nothing edited"
before=$(snap)
inst < /dev/null >/dev/null 2>&1 || fail "non-interactive install failed"
[ "$(snap)" = "$before" ] || fail "a non-interactive run edited a file"
inst --no-profile >/dev/null 2>&1 || fail "install --no-profile failed"
[ "$(snap)" = "$before" ] || fail "--no-profile edited a file"
echo n | MASON_INSTALL_PROMPT=1 inst >/dev/null 2>&1 || fail "install answered n failed"
[ "$(snap)" = "$before" ] || fail "answering n edited a file"

echo "== yes: one block in .bash_profile, replaced on a second run"
out=$(echo y | MASON_INSTALL_PROMPT=1 inst 2>&1) || fail "install answered y failed"
echo "$out" | grep -q "$home/.bash_profile so" || fail "the question did not name the file: $out"
inst --profile >/dev/null 2>&1 || fail "install --profile failed"
[ "$(grep -c '^# >>> mason >>>$' "$home/.bash_profile")" = 1 ] || fail "not exactly one block"
[ "$(grep -c '^# <<< mason <<<$' "$home/.bash_profile")" = 1 ] || fail "not exactly one end marker"
grep -qx 'export FOO=1' "$home/.bash_profile" || fail "text outside the block was lost"
[ "$(cat "$home/.profile")" = 'export BAR=2' ] || fail ".profile was touched"
# an edited block is restored in place, and text after it is kept
sed -i.bak 's/Added by MASON.*/stale/' "$home/.bash_profile"; rm -f "$home/.bash_profile.bak"
echo 'export AFTER=3' >> "$home/.bash_profile"
inst --profile >/dev/null 2>&1 || fail "third install failed"
grep -q stale "$home/.bash_profile" && fail "the block was not replaced"
[ "$(grep -c '^# >>> mason >>>$' "$home/.bash_profile")" = 1 ] || fail "block duplicated"
[ "$(tail -n 1 "$home/.bash_profile")" = 'export AFTER=3' ] || fail "text after the block moved"

echo "== zsh and others"
env HOME="$home" SHELL=/bin/zsh bash scripts/install.sh --version 9.9.9 --base-url "file://$rel" --prefix "$home/.local" --profile >/dev/null 2>&1
[ -f "$home/.zprofile" ] || fail "zsh did not get .zprofile"

echo "== the function runs the current runtime"
case "$(uname -s)" in Darwin) data="$home/Library/Application Support/mason" ;; *) data="$home/.local/share/mason" ;; esac
mkdir -p "$data/runtimes/r1" "$data/runtimes/r2" "$home/.config/mason/bin"
for r in r1 r2; do printf '#!/bin/sh\necho "runtime %s $*"\n' "$r" > "$data/runtimes/$r/mason"; chmod +x "$data/runtimes/$r/mason"; done
printf '#!/bin/sh\necho "fallback $*"\n' > "$home/.config/mason/bin/mason"; chmod +x "$home/.config/mason/bin/mason"
echo r1 > "$data/runtimes/current"
run() { env -i HOME="$home" PATH=/usr/bin:/bin bash -c '. "$HOME/.bash_profile"; mason "$@"' x "$@"; }
[ "$(run --version)" = "runtime r1 --version" ] || fail "did not run r1: $(run --version)"
echo r2 > "$data/runtimes/current"
[ "$(run a b)" = "runtime r2 a b" ] || fail "did not follow current to r2"
rm "$data/runtimes/current"
[ "$(run x)" = "fallback x" ] || fail "no fallback without runtimes/current"
echo "PASS: s331"
