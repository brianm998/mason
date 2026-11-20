#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s671 exit test: s634.sh turns the Rust-style pass_regex into an ERE before grep -E, and the
# translated regex matches libtest result lines. No cargo, no timing assertions.
set -euo pipefail
cd "$(dirname "$0")/../.."
pass=$(sed -n "s/^pass_regex = '\(.*\)'$/\1/p" .mason/mason.toml | head -1)
ere=${pass//'(?:'/'('}
for l in 'test tests::quick ... ok <0.000s>' \
  'test tests::waits_for_a_file ... ok <1.234s>' \
  'test src/lib.rs - one (line 1) ... ok <0.012s>' \
  'test tests::quick ... ok'; do
  echo "$l" | grep -Eq -- "$ere" || { echo "s671: no match: $l" >&2; exit 1; }
done
if echo 'test tests::quick ... FAILED <0.1s>' | grep -Eq -- "$ere"; then
  echo "s671: FAILED line matched" >&2; exit 1
fi
grep -q 'ere=${pass//' scripts/exit/s634.sh || { echo "s671: s634.sh lacks the translation" >&2; exit 1; }
if grep -E 'grep -Eq? .*"\$pass"' scripts/exit/s634.sh; then
  echo "s671: s634.sh passes the raw regex to grep -E" >&2; exit 1
fi
echo "s671: ok"
