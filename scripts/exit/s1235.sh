#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1235 exit test: a batch lands through a remote that rejects the first push (decision 1225).
set -euo pipefail
cd "$(dirname "$0")/../.."

grep -q '^Summary: ' docs/stages/s1235.md
cargo test -q -p mason-landing --test batch a_batch_push_the_remote_rejects_once_is_tried_again
cargo test -q -p mason-landing --test direct rejects
cargo test -q -p mason-landing --test direct a_refusal_is_not_tried_again
echo "OK s1235"
