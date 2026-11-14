#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s248 exit test: scripts/test-parallel.py runs a workspace's test executables side by side, keeps
# cargo's own lines in one block per executable, runs the serial group alone and the doc tests
# last, and exits non-zero when any executable fails. A tiny fixture workspace has one passing
# test binary, one failing one and a doc test.
set -euo pipefail
cd "$(dirname "$0")/../.."
root=$PWD
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
fx="$tmp/fx"
mkdir -p "$fx/crates/pkg/src" "$fx/crates/pkg/tests" "$fx/.mason"
cat > "$fx/Cargo.toml" <<'T'
[workspace]
members = ["crates/pkg"]
resolver = "2"
T
cat > "$fx/crates/pkg/Cargo.toml" <<'T'
[package]
name = "pkg"
version = "0.1.0"
edition = "2021"
T
cat > "$fx/crates/pkg/src/lib.rs" <<'T'
/// ```
/// assert_eq!(pkg::two(), 2);
/// ```
pub fn two() -> u32 { 2 }
T
cat > "$fx/crates/pkg/tests/good.rs" <<'T'
#[test]
fn good_one() { assert_eq!(pkg::two(), 2); }
#[test]
fn runs_in_its_package_dir() {
    let want = std::fs::canonicalize(std::env::var("CARGO_MANIFEST_DIR").unwrap()).unwrap();
    let have = std::fs::canonicalize(std::env::current_dir().unwrap()).unwrap();
    assert_eq!(want, have);
    assert!(want.ends_with("crates/pkg"));
}
T
cat > "$fx/crates/pkg/tests/bad.rs" <<'T'
#[test]
fn bad_one() { assert_eq!(pkg::two(), 3); }
T
printf '# run alone, after the others\ngood\n' > "$fx/.mason/serial-tests.txt"
export CARGO_TARGET_DIR="$tmp/target"
unset MASON_TEST_JOBS
run() { (cd "$fx" && python3 -I "$root/scripts/test-parallel.py" --jobs 2 -- cargo test --workspace "$@"); }

echo "== a failing binary fails the run"
out=$(run 2>&1) && { echo "$out" >&2; echo "FAILED: the run passed with a failing test binary" >&2; exit 1; }
for want in "     Running tests/good.rs (" "     Running tests/bad.rs (" "     Running unittests src/lib.rs (" \
            "test good_one ... ok" "test runs_in_its_package_dir ... ok" "test bad_one ... FAILED" \
            "Doc-tests" "lib.rs - two (line 1) ... ok" \
            "test-parallel: 3 executables (1 serial)" "1 failed" ; do
    echo "$out" | grep -qF -- "$want" || { echo "$out" >&2; echo "FAILED: output lacks: $want" >&2; exit 1; }
done
# One block per binary: the header comes before that binary's own lines.
echo "$out" | awk '/Running tests\/bad.rs/{b=1} /test bad_one/{ if(!b){exit 1} }' \
    || { echo "FAILED: bad_one is not under its own header" >&2; exit 1; }
# The serial group runs after the parallel ones.
g=$(echo "$out" | grep -n "Running tests/good.rs" | head -1 | cut -d: -f1)
b=$(echo "$out" | grep -n "Running tests/bad.rs" | head -1 | cut -d: -f1)
[ "$g" -gt "$b" ] || { echo "FAILED: the serial binary did not run last" >&2; exit 1; }

echo "== a name filter goes to every binary"
out=$(run good_one 2>&1) || { echo "$out" >&2; echo "FAILED: the filtered run failed" >&2; exit 1; }
echo "$out" | grep -qF "test good_one ... ok" || { echo "FAILED: filter dropped good_one" >&2; exit 1; }
echo "$out" | grep -qF "bad_one" && { echo "FAILED: filter was not passed on" >&2; exit 1; }

echo "== all green: exit 0, and the count matches cargo's own"
rm "$fx/crates/pkg/tests/bad.rs"
out=$(run 2>&1) || { echo "$out" >&2; echo "FAILED: the green run failed" >&2; exit 1; }
mine=$(echo "$out" | grep -cE '^test .* \.\.\. ok$' || true)
ref=$(cd "$fx" && cargo test --workspace 2>&1 | grep -cE '^test .* \.\.\. ok$' || true)
[ "$mine" = "$ref" ] && [ "$mine" -ge 3 ] || { echo "FAILED: $mine passing lines, cargo gives $ref" >&2; exit 1; }
echo "$out" | grep -qF "0 failed" || { echo "FAILED: no summary" >&2; exit 1; }

echo "== no executable: not a pass"
(cd "$fx" && python3 -I "$root/scripts/test-parallel.py" -- cargo test --workspace --lib --bins --exclude pkg >/dev/null 2>&1) \
    && { echo "FAILED: a run that found nothing passed" >&2; exit 1; }
echo "s248 OK"
