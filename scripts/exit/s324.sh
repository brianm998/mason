#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s324 exit test: spoken alerts are short, in the new order, with no all-capitals words.
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test -q -p mason-core --lib -- notify::tests::spoken_subject_is_project_issue notify::tests::no_all_capitals
echo "s324 OK"
