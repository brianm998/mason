#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s693 exit test: a landing batch merges the head a task submitted, not its live branch.
set -euo pipefail
cd "$(dirname "$0")/../.."
# The first test asserts the post-submit commit is absent from the integration branch's log.
cargo test -q -p mason-landing --test batch a_commit_made_after_the_task_was_pinned_does_not_join_the_batch
cargo test -q -p mason-landing --test batch a_head_that_differs_from_the_tested_one_is_reported_moved
echo "s693: ok"
