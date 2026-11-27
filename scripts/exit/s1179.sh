#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1179 exit test: a review finding quoting a line not in the diff is refused; the same session
# resubmits with a real line and is accepted, and `mason task show` prints the quote.
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test -q -p mason-adapter-api --lib -- quote
cargo test -q -p mason-daemon --test it -- review_severity
echo "s1179 passed"
