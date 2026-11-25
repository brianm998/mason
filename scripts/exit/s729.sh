#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s729 exit test: a suite split across runners (#701, decision 0722).
#  1. Against a fake GitHub endpoint that answers a 3-way matrix, the merged verdict and test
#     count equal the union of the three artifacts, one red shard is red, and a shard that never
#     got an artifact is made again alone (daemon unit tests, in-process fake endpoint).
#  2. scripts/test-shard.py over a small scratch crate, as shard 2 of 3, runs exactly the tests
#     assigned to it (counted from its output lines, not timed); the three shards together run
#     every test once, and a test in no list runs on the shard its name hashes to.
set -euo pipefail
cd "$(dirname "$0")/../.."
root=$PWD

echo "== merge: three artifacts are one result"
cargo test -q -p mason-daemon --lib -- \
    remote::tests::three_shards_are_one_result_in_shard_order \
    remote::tests::one_red_shard_makes_the_merge_red \
    remote::tests::a_shard_that_never_ran_is_made_again_alone \
    suites::remote_tests::three_shards_are_judged_as_the_one_combined_output \
    suites::remote_tests::a_red_shard_names_its_failing_tests \
    shards::tests

echo "== scripts/test-shard.py over a scratch crate"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
fx="$tmp/fx"
mkdir -p "$fx/src" "$fx/tests" "$fx/.mason"
cat > "$fx/Cargo.toml" <<'T'
[package]
name = "scratch"
version = "0.1.0"
edition = "2021"
T
cat > "$fx/src/lib.rs" <<'T'
#[cfg(test)]
mod tests {
    #[test] fn u1() {}
    #[test] fn u2() {}
    #[test] fn u3() {}
}
T
cat > "$fx/tests/it.rs" <<'T'
#[test] fn i1() {}
#[test] fn i2() {}
#[test] fn i3() {}
#[test] fn brand_new() {}
T
# u1, i1 on shard 1; u2, i2 on shard 2; u3, i3 on shard 3. `brand_new` is in no list.
cat > "$fx/.mason/run.json" <<'T'
{"shards": 3, "shard_lists": [["tests::u1", "i1"], ["tests::u2", "i2"], ["tests::u3", "i3"]]}
T
export CARGO_TARGET_DIR="$tmp/target"
shard() { (cd "$fx" && python3 -I "$root/scripts/test-shard.py" --shard "$1" -- cargo test 2>&1); }
names() { sed -n 's/^test \(.*\) \.\.\. ok.*$/\1/p' | sort | tr '\n' ' '; }

# The hash shard of `brand_new`, computed here independently of the script (FNV-1a 64).
hashed=$(python3 -I -c '
h = 0xCBF29CE484222325
for b in b"brand_new":
    h = ((h ^ b) * 0x100000001B3) & 0xFFFFFFFFFFFFFFFF
print(h % 3 + 1)')

all=""
for k in 1 2 3; do
    out=$(shard "$k") || { echo "$out" >&2; echo "FAILED: shard $k exited non-zero" >&2; exit 1; }
    got=$(echo "$out" | names)
    case $k in
        1) want="i1 tests::u1" ;;
        2) want="i2 tests::u2" ;;
        3) want="i3 tests::u3" ;;
    esac
    [ "$k" = "$hashed" ] && want="$want brand_new"
    want=$(printf '%s\n' $want | sort | tr '\n' ' ')
    [ "$got" = "$want" ] || { echo "$out" >&2; echo "FAILED: shard $k ran [$got], wanted [$want]" >&2; exit 1; }
    echo "$out" | grep -qF "test-shard: shard $k of 3" || { echo "$out" >&2; echo "FAILED: no summary line" >&2; exit 1; }
    all="$all$got"
done
n=$(echo "$all" | tr ' ' '\n' | grep -c .)
[ "$n" = 7 ] || { echo "FAILED: the shards ran $n tests together, wanted 7 (each once)" >&2; exit 1; }
[ "$(echo "$all" | tr ' ' '\n' | grep . | sort | uniq -d | wc -l)" -eq 0 ] \
    || { echo "FAILED: a test ran on two shards" >&2; exit 1; }
echo "ok: shard 2 of 3 ran exactly its tests; the three shards ran all 7 once"
