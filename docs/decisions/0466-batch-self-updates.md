# 0466: Batch self-updates with a minimum gap, and build under a compile lease

Status: accepted, waits for the owner's approval (`update.rs` is a protected file).

Decision. A follower's update request (`auto`) is held until `min_gap_s` (default 2 h) after the last takeover, unless the update is marked urgent (`Update-Urgent:` in a commit message), no pipeline task is running, or, from half the gap on, the machine is quiet. The owner's `mason update` ignores the gap. The release build holds a `compile` lease ranked as the lowest pipeline work.

Why. 18 restarts in 24 h, each a takeover and a 5-minute unqueued build.

Alternatives. Gating in the checker was not possible: nothing in the repository takes updates automatically. Ranking the build as a person's request would have put it ahead of task work.

Not verified: see docs/stages/s473.md.
