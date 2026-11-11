# 0086: A full disk is announced as a stop, and every held request names it

**Decision.** `disk` becomes a routed condition with the channels of `failed` and `landing` (monitor, sound and speech now, push after 10 minutes), listed in `mason notify status`. `mason status` and `--json` show the hold only while it is true. In the plan, the disk cause is checked before the queue causes, so each request the disk holds says "disk", not a queue for a compile slot.

**Why.** On 2026-11-09 eight of nine waiting tasks named the wrong cause and the one accurate line sat in `mason alerts`. Only disk is made to alert: CPU and memory waits resolve as work finishes.

**Choices.**
- Reordering the arms changes only the stated cause. The request is still added to the blocked set and the ahead list exactly as before, so grants and positions are identical (tested against the same world with room).
- A request that cannot fit at all (`Fit::Never`/`Full`) keeps its own cause; the disk is not blamed for that.
- The count of held tasks is taken from the live plan, not stored, so it is right as soon as the disk reading changes.
- Email is not added: a full disk is the owner's to clear but not an emergency of the daemon-down kind.

**Trust files.** `crates/scheduler/**` and `crates/daemon/src/attention.rs` are touched, and `sched.rs` gains a read-only counter. Only the stated wait cause and alert text change: no budget, limit, grant or who-may-answer decision. Owner approval is required for those paths.
