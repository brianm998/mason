#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1106 exit test: the content scan reports an added token and an added banned word, and its
# output holds neither the token nor the list (decision 1095).
set -euo pipefail
cd "$(dirname "$0")/../.."

grep -q '^Summary: ' docs/stages/s1106.md
cargo build -q -p mason-daemon --bin masond
bin="${CARGO_TARGET_DIR:-$PWD/target}/debug/masond"

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
repo="$tmp/repo"
git init -q -b main "$repo"
g() { git -C "$repo" -c user.name=t -c user.email=t@t -c commit.gpgsign=false "$@"; }
echo base >"$repo/f.txt"
g add -A
g commit -qm base
g checkout -q -b work
# Put together here, so this file holds no token.
token="ghp_$(printf 'e5%.0s' 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18)"
printf 'GH=%s\nsay QuaggaWord here\n' "$token" >>"$repo/f.txt"
g commit -qam work
printf '# the owner list\nquaggaword\nunused-list-entry\n' >"$tmp/words.txt"

status=0
out="$(MASON_BANNED_WORDS="$tmp/words.txt" "$bin" contentscan \
    --repo "$repo" --base main --config-dir "$tmp/config" 2>&1)" || status=$?
echo "$out"
[ "$status" -eq 1 ]
grep -q 'f.txt:2 (GitHub token)' <<<"$out"
grep -q 'f.txt:3 (banned word)' <<<"$out"
for hidden in "$token" quaggaword unused-list-entry 'the owner list'; do
    if grep -qiF -- "$hidden" <<<"$out"; then
        echo "the output holds text it must mask" >&2
        exit 1
    fi
done

cargo test -q -p mason-daemon --lib contentscan::
cargo test -q -p mason-daemon --lib submit_refuses_a_branch_that_adds_a_token
cargo test -q -p mason-daemon --test it a_branch_that_adds_a_secret_or_a_banned_word
echo "OK s1106"
