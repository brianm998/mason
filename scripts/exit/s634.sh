#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s634 exit test: a scratch workspace with two tests, one of which waits on a condition (a file
# another thread writes; no fixed sleep). Run the way .mason/mason.toml runs the cargo suite
# (RUSTC_BOOTSTRAP=1, `-- -Z unstable-options --report-time`), each test's result line carries a
# `<N.NNNs>` time and the suite's own pass_regex/fail_regex still match it. Then the parser,
# store and report tests that turn those times into `mason report`'s slowest-tests list run.
set -euo pipefail
cd "$(dirname "$0")/../.."
root=$PWD
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
fx="$tmp/fx"
mkdir -p "$fx/src"
cat > "$fx/Cargo.toml" <<'T'
[package]
name = "scratch"
version = "0.1.0"
edition = "2021"
T
cat > "$fx/src/lib.rs" <<'T'
#[cfg(test)]
mod tests {
    #[test]
    fn quick() {}
    #[test]
    fn waits_for_a_file() {
        let f = std::env::temp_dir().join(format!("s634-{}", std::process::id()));
        let g = f.clone();
        std::thread::spawn(move || std::fs::write(g, "x").unwrap());
        while !f.exists() { std::thread::yield_now(); }
        let _ = std::fs::remove_file(f);
    }
}
T
out="$tmp/out"
(cd "$fx" && RUSTC_BOOTSTRAP=1 cargo test -- -Z unstable-options --report-time) > "$out" 2>&1 || { cat "$out" >&2; exit 1; }
pass=$(sed -n "s/^pass_regex = '\(.*\)'$/\1/p" .mason/mason.toml | head -1)
for t in quick waits_for_a_file; do
  line=$(grep -E "^test tests::$t \.\.\. ok" "$out") || { echo "no result line for $t" >&2; cat "$out" >&2; exit 1; }
  echo "$line" | grep -Eq '<[0-9]+\.[0-9]+s>$' || { echo "no time on: $line" >&2; exit 1; }
  echo "$line" | grep -Eq -- "$pass" || { echo "pass_regex does not match: $line" >&2; exit 1; }
done
cd "$root"
cargo test -p mason-testing report_time
cargo test -p mason-store --test test_times
cargo test -p mason-daemon --lib every_section_counts_a_small_synthetic_log
echo "s634 ok"
