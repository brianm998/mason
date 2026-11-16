#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s489 exit test: the dev profile sets split-debuginfo, the decision has measured rows, and a
# panicking test still names file and line in its backtrace.
set -uo pipefail
cd "$(dirname "$0")/../.."
grep -Eq '^split-debuginfo = "(packed|off)"' Cargo.toml || { echo "s489: split-debuginfo not set"; exit 1; }
d=docs/decisions/0482-split-debuginfo-packed.md
for w in unpacked packed off; do
  grep -Eq "^\| $w +\| [0-9]+ " "$d" || { echo "s489: no measured row for $w"; exit 1; }
done
cargo test -q -p mason-core --test backtrace_lines || exit 1
echo "s489: ok"
