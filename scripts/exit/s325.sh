#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s325 exit test: a deferred task stays out of the queue and the alerts; approved with a priority it is queued at it.
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test -q -p mason-engine --test behaviour -- deferred_is_not_admitted a_priority_chosen_at_approval
cargo test -q -p mason-store codec
cargo test -q -p mason-daemon --test attention -- a_deferred_task_raises_no_alert
echo "s325 OK"
