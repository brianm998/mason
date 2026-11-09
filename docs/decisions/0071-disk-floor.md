# 0071: A free-disk floor holds new work

**Decision.** `[disk] floor_gb` in `resources.toml`, default 20. Below it the broker grants no `compile` and no LLM session; everything else, work already running, and landing go on. Unlike the memory floor it applies on an idle machine too, because the failure it prevents (the store and the build both failing with "No space left") needs no other work to happen.

**How 20 GB was chosen.** The default must not hold back a healthy machine. The two things that fill a disk are a build directory (4 to 11 GB each, from #54) and a worktree; a task in flight needs room for about one of each plus the store's growth, so one task needs roughly 15 GB and 20 GB leaves a margin of one more small build. On the owner's machine the disk had 138 GB free on 2026-11-09 and was at 0 on 2026-11-08, so 20 GB is far below any healthy reading and far above the point where builds fail. A machine whose volume is smaller than ~100 GB can set a lower floor, or 0 to turn it off. The floor is read at most every 30 s, and a reading that cannot be taken means no gate rather than a held machine. This is a judgement from the figures above, not a measurement of how much a task needs.

**Other choices.**
- Free space is the least over the data directory and each project's `task_cache` volume, since either filling breaks MASON.
- The gate lives in the scheduler as one more reason to wait (`WaitCause::Disk`), placed after the existing reasons so queue order explanations are unchanged. It never revokes a lease.
- The alert is computed live in `alerts_now` and so clears itself. A refused store write because of a full disk raises it too, and the next successful write clears it.
- `task_cache_keep_days` (default 7) ages out cancelled and failed tasks' directories from their last step; a revived task rebuilds, which is only slower.

**Trust files.** `crates/daemon/src/sched.rs`, `crates/scheduler/**` (budgets) and `attention.rs` (trust file) change. Each only adds a reason to wait or an alert: no limit, gate or verdict is loosened. Owner approval is needed for those paths.
