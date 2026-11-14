# 0255: waiting jobs share one scheduling pass per 100 ms

**Decision.** A waiter in `Broker::acquire` runs `pass` only if no pass has run in the last `PASS_EVERY` (100 ms). The pass, the `holds` check and the wait stay under the broker lock. The home view reads model-session holders with one lock (`Broker::llm_holders`).

**Why.** W waiters each passing ten times a second kept the broker mutex held nearly all the time (41 waiters on 2026-11-13), starving monitor views and `mason run` sessions until their WebSockets were reset. One pass already grants everything that can be granted, and a grant wakes every waiter, so the others only need to check their own lease.

**Alternatives.** #284 also moves lock-file probes outside the lock. That is a larger change still in development, and releasing the lock mid-pass needs care so grants are not doubled. This decision does not preclude it.

**Trust.** `crates/daemon/src/sched.rs` (scheduler) and `scripts/exit/s260.sh` (exit tests) are in `PROTECTED`; `views.rs` is not. The owner approved this change on 2026-11-13 ("Yes, fix and land"). No gate, limit, ranking or test is weakened. (Corrected in s271: the first version said none of these paths was protected.)
