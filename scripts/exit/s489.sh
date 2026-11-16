#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s489 exit test: the dev profile sets split-debuginfo and the decision has measured rows.
# The backtrace test (crates/core/tests/backtrace_lines.rs) runs in cargo test --workspace.
set -uo pipefail
cd "$(dirname "$0")/../.."
grep -Eq '^split-debuginfo = "(packed|off)"' Cargo.toml || { echo "s489: split-debuginfo not set"; exit 1; }
d=docs/decisions/0482-split-debuginfo-packed.md
for w in unpacked packed off; do
  grep -Eq "^\| $w +\| [0-9]+ " "$d" || { echo "s489: no measured row for $w"; exit 1; }
done
echo "s489: ok"
