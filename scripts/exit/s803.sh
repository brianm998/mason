#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s803 exit test: a landing check never sends a suite's raw build template to GitHub.
set -uo pipefail
cd "$(dirname "$0")/../.."
cargo test -q -p mason-daemon --lib build_choice_tests || exit 1
if grep -n 'Box::new(suite.clone())' crates/daemon/src/landing.rs; then
  echo "a landing check passes the suite with its raw build"; exit 1
fi
echo "ok"
