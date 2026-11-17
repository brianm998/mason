# 0520: what a task waits on is published as data, built with its sentence

**Decision.** Each waiting task carries a structured wait (cause code, resource, place in line, holders, the request it may not pass, the whole chain) made from the same scheduler `Wait` as its sentence. The home view carries one `QueueHold` when most waiting work is held by one exclusive request, one gate or one red test. Leases that belong to no task are listed with their origin.

**Why.** A request held by the passing rule behind a waiting exclusive request was described by the resource it wanted, so rows said "waiting for compile slot, held by #510" with slots free (2026-11-16). One sentence per task cannot name a request that is not a holder.

**Choices.**
- The blocker is set only when the passing rule held the request (a touched reservation), never for a full resource, a gate or a size, so the holder stays the answer there.
- "Most" is more than half of the waiting tasks and at least two, so one slow task is not a banner.
- The view is written in the same pass as the sentence snapshot, from the same plan, so a holder that releases is gone from every row on the next pass (#507).
- Origin is a free string from `--origin` plus the working directory; the daemon never trusts it for ranking.

**Trust.** `crates/daemon/src/sched.rs`, `crates/scheduler/**` and `clients/features.toml` are in `PROTECTED`; this waits for the owner's approval. No gate, limit or budget changes: the passing rule and its results are as before; only descriptions were added.
