#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s527 exit test: what each task waits on is data (the real blocker, a queue-held summary, the
# leases that belong to no task), and both clients show it. The broker and view tests always
# run; the Swift and Compose tests run when swift / gradle are installed (MASON_SKIP_SWIFT or
# MASON_SKIP_COMPOSE skips one).
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test -q -p mason-scheduler --lib -- explain::tests
cargo test -q -p mason-daemon --lib -- sched::tests::ten_compiles_behind_a_waiting_exclusive_lease_all_name_it sched::tests::a_gate_holding_most_waiting_work_is_the_queue_summary
cargo test -q -p mason-daemon --lib -- waitview
cargo test -q -p mason-cli -- the_hook_wraps_builds_and_leaves_everything_else

if [ "$(uname)" = Darwin ] && command -v swift >/dev/null && [ -z "${MASON_SKIP_SWIFT:-}" ]; then
  (cd clients/swift && swift test --filter QueueWaitTests --filter QueueHoldBannerTests)
fi
if command -v java >/dev/null && command -v gradle >/dev/null && [ -z "${MASON_SKIP_COMPOSE:-}" ]; then
  (cd clients/compose && gradle --console=plain :core:test --tests '*QueueWaitTest' :app:test --tests '*QueueHoldBannerTest')
fi
grep -q 'behind-exclusive' schemas/mason.proto || grep -q 'queue_hold' schemas/mason.proto
echo "s527 OK"
