#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s703: a src change one module of a directory test target references runs only that module.
set -euo pipefail
cd "$(dirname "$0")/../.."
repo=$PWD
scratch=$(mktemp -d)
trap 'rm -rf "$scratch"' EXIT
cd "$scratch"
mkdir -p crates/a/src crates/a/tests/it
printf '[workspace]\nresolver = "2"\nmembers = ["crates/*"]\n' > Cargo.toml
printf '[package]\nname = "a"\nversion = "0.1.0"\nedition = "2021"\n' > crates/a/Cargo.toml
printf 'pub mod shapes;\n' > crates/a/src/lib.rs
printf 'pub fn area() -> u32 { 1 }\n' > crates/a/src/shapes.rs
printf 'mod m1;\nmod m2;\nmod m3;\n' > crates/a/tests/it/main.rs
printf '#[test]\nfn one() { assert_eq!(a::shapes::area(), 1); }\n#[test]\nfn two() {}\n' > crates/a/tests/it/m1.rs
for m in m2 m3; do printf '#[test]\nfn one() {}\n#[test]\nfn two() {}\n' > crates/a/tests/it/$m.rs; done
cargo generate-lockfile --offline -q
git init -q . && git add -A

r=$(cd "$repo" && MASON_NARROW_PROBE_ROOT="$scratch" MASON_NARROW_PROBE_PATHS="crates/a/src/shapes.rs" \
  cargo test -q -p mason-daemon --lib scope::tests::narrow_probe -- --exact --nocapture 2>&1 \
  | grep '^NARROW:' | head -1)
echo "$r"
case $r in *"-p a --test it -- m1::"*) ;; *) echo "FAIL: m1 not selected as a filtered group"; exit 1;; esac
case $r in *m2*|*m3*) echo "FAIL: other modules selected"; exit 1;; esac
build=$(cargo test -q --offline -p a --test it --no-run 2>&1)
case $build in *"running "*) echo "FAIL: the build ran tests"; exit 1;; esac
out=$(cargo test --offline -p a --test it -- m1:: 2>&1)
echo "$out" | grep 'test result'
echo "$out" | grep -q 'test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 4 filtered out' \
  || { echo "FAIL: expected exactly m1's 2 tests to run"; exit 1; }
echo "s703 OK"
