#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test -q -p mason-core --lib -- ci_watch::tests::excerpt ci_watch::tests::a_red_job_whose_next_run ci_watch::tests::a_cancelled_job_leaves
cargo test -q -p mason-daemon --lib -- the_filed_text_has_the_error_line
echo "s701 OK"
