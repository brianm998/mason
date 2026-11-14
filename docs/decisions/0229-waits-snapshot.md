# 0229: wait reasons are published per pass, not computed per row

`explain_task` took the broker lock and ran the disk, memory and CPU gates plus `waits_gated` over the whole ledger for each row, so a sidebar build was O(waiting²) under the busiest lock in the daemon, and a fresh monitor connection was closed after `VIEW_WAIT`.

**Decision.** `Broker::pass`, which every submit, release, grant and `lease_*` ends in, calls `publish_waits`, which stores one `Arc<BTreeMap<TaskId, String>>` computed from a single `waits_gated`. `explain_task` and `waits_snapshot` read it without `st`. A reason may be one pass old; `acquire`'s own `on_wait` text is still live. A thread-local counter (`broker_locks_taken_by_this_thread`) lets a test assert lock use per build. The scheduler crate is untouched. The companion pass-loop task should call `publish_waits` from its single pass.

**Trust.** None of the changed files is protected; no gate or test is weakened.

**Not verified:** the live 36-job case; the added `waits_gated` per pass is O(waiting) and not measured.
