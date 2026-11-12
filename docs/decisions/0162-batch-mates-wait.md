# 0162: A task that conflicts only with batch-mates waits for the next batch

In star's batch #103, #159 merged first; #160 and #171 conflicted with it and went back to develop. Then #159 went red and was parked, so nothing landed, and both tasks were re-developed against a change that never reached the base.

**Rule.** When assembling, a task whose merge conflicts (and the resolver cannot or does not resolve it) is tried against the base tip alone (`git merge-tree --write-tree`). If that merges cleanly and at least one task was already merged into the batch, the task is *deferred*: `Assembled.deferred`, with the mates it conflicts with (the merged tasks whose own diff touches a conflicting file; all merged tasks if none does). Otherwise it is ejected as before and goes back to develop with the files named.

**Daemon.** A deferred task is given no verdict. Its activity says `waits: conflicts with #N in batch #B (files)`. When the batch ends, however it ends, the task's `Wait` is put back in the queue unchanged, so it keeps its `ready_at` and rank: it goes first if its mate did not land. When the next batch assembles, a task that now conflicts with the base (the mate landed) is ejected with the feedback naming the files.

**Restart.** The queue is in memory; the landing step of a waiting task is still open, so after a restart the task is submitted again and waits again. Nothing was written that a restart could lose or repeat. The one thing not preserved is `ready_at` (the window restarts).

**A deferral is not a round.** The batch's round limit counts rounds, not deferrals; a deferred task is out of `active` for the rest of the batch, including after a red that parks its mate.

**Trust.** `crates/landing/**` and `crates/daemon/src/landing.rs` are protected (how work is merged; batching). The change needs the owner's approval.
