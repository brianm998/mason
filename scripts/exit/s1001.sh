#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1001 exit test: speech announcements fold into one summary (tests run in the test step).
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
grep -q 'pub fn summary_sentence' crates/core/src/notify.rs
grep -q 'fn fold_for_speech' crates/daemon/src/notify.rs
grep -q 'summarize_from' clients/features.toml
echo "s1001 passed"
