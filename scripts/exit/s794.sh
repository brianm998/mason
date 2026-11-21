#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s794 exit test: a coloured lint red is blamed by file, not bisected.
set -uo pipefail
cd "$(dirname "$0")/../.."
if cargo test -q -p mason-daemon --lib lint_parsers_read_coloured_output >/dev/null 2>&1 \
  && cargo test -q -p mason-daemon --lib a_file_far_from_the_end >/dev/null 2>&1 \
  && cargo test -q -p mason-daemon --test it landing::a_coloured_clippy_red >/dev/null 2>&1; then
  echo "PASS s794"
else
  echo "FAIL s794"
  exit 1
fi
