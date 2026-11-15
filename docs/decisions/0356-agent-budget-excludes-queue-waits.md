# 0356: an agent's budget does not count its queue waits

**Decision.** Time an agent's own `mason run` commands wait in the resource queue is added to the agent step's deadline when each is granted. This extends what `run_check` already did (its queue time was added since `DeadlineExtra` was introduced) to every `mason run` the agent makes for its task.

**Why.** A step's budget is meant to bound the agent's work, not the machine's queue. Since the order is "further along first" (s349), a develop agent's heavy commands wait behind everything nearer landing. On a busy day that wait alone can spend a 4-hour budget, which failed star #242 (2026-11-14). The time is credited only when a lease is granted, so an agent cannot stretch its budget by asking for something it never gets.

**Trust.** `crates/daemon/src/sched.rs` and `scripts/exit/s363.sh` are in `PROTECTED`. The owner asked for #242 to be looked into on 2026-11-14 ("look at #242"). No limit, capacity or gate changes. The budget still bounds the agent's working time.
