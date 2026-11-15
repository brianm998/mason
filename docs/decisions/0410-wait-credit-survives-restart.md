# 0410: queue-wait credit is added as it goes and kept on disk

**Decision.** The time an agent's `mason run` waits in the queue is added to its deadline every 30 seconds while it waits, not only once it is granted. The running total is kept in the task's folder for that run, so a daemon that restarts and re-attaches to the session goes on from it.

**Why.** A deadline already survives a restart, because it counts from the step host's start. The credit did not, so every restart charged all open waits to the agent as working time. On 2026-11-15 a self-update timed out 29 develop agents that had been waiting for the machine (s417).

**Trust.** `crates/daemon/src/sched.rs` ("budgets: leases and limits") and `scripts/exit/s417.sh` are in `PROTECTED`. The owner approved this on 2026-11-15 ("Yes, fix it here"). An agent's budget for its own work is unchanged; it no longer loses its waiting time.
