#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s665: a changed file outside a crate's src/ selects only the test targets that read it.
set -euo pipefail
cd "$(dirname "$0")/../.."
repo=$PWD
scratch=$(mktemp -d)
trap 'rm -rf "$scratch"' EXIT
cd "$scratch"
mkdir -p crates/a/src crates/a/tests/support crates/b/src crates/b/tests
printf '[workspace]\nresolver = "2"\nmembers = ["crates/*"]\n' > Cargo.toml
for c in a b; do
  printf '[package]\nname = "%s"\nversion = "0.1.0"\nedition = "2021"\n' $c > crates/$c/Cargo.toml
  : > crates/$c/src/lib.rs
done
: > crates/a/tests/support/mod.rs
printf 'mod support;\n#[test]\nfn t() {}\n' > crates/a/tests/uses_support.rs
printf '#[test]\nfn t() {}\n' > crates/a/tests/plain.rs
printf '#[test]\nfn t() { include_str!("../../../data.toml"); }\n' > crates/b/tests/reads_data.rs
printf '#[test]\nfn t() {}\n' > crates/b/tests/other.rs
printf 'x = 1\n' > data.toml
cargo generate-lockfile --offline -q
git init -q . && git add -A

probe() { # paths...
  (cd "$repo" && MASON_NARROW_PROBE_ROOT="$scratch" MASON_NARROW_PROBE_PATHS="$*" \
    cargo test -q -p mason-daemon --lib scope::tests::narrow_probe -- --exact --nocapture 2>&1 \
    | grep '^NARROW:' | head -1)
}
want() { # what, got, must-contain, must-not-contain
  echo "$1 -> $2"
  case $2 in *"$3"*) ;; *) echo "FAIL: $1 lacks $3"; exit 1;; esac
  if [ -n "${4:-}" ]; then case $2 in *"$4"*) echo "FAIL: $1 has $4"; exit 1;; esac; fi
}
r=$(probe crates/a/tests/support/mod.rs)
want support "$r" "-p a --test uses_support" "plain"
case $r in *"-p b"*) echo "FAIL: support selected b"; exit 1;; esac
r=$(probe data.toml)
want data.toml "$r" "-p b --test reads_data" "other"
case $r in *"-p a"*) echo "FAIL: data.toml selected a"; exit 1;; esac
r=$(probe Cargo.lock)
want Cargo.lock "$r" "runs whole because \`Cargo.lock\` changed"
echo "s665 OK"
