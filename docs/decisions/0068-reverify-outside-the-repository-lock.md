# 0068: Landing re-verifies outside the repository lock

When the base moves while a batch is landing, `Batch::finish_with` folds it in under the repository lock, releases the lock, re-verifies, and takes the lock again to promote. The lock is held only around git.

**How a batch is still verified on the exact tree it promotes.** The integration worktree belongs to the batch's slot, so the tree being verified does not change during the re-verification. The loop keeps `seen`, the base tip the worktree's tree was last verified on (set at the fold, initially the tip the batch was assembled and first verified on). Under the lock, the promote step fetches and reads the base again: if it is not `seen`, nothing is promoted; the loop goes round, folds the new tip, and re-verifies, counting against `max_fold` (`BaseKeptMoving` when it runs out). Only when the tip equals `seen` does it push (or compare-and-swap the local ref), and the push is fast-forward only, so the base cannot have moved between the compare and the push without the push being refused (`Moved`, which also goes round again). The promote reads HEAD under the same lock it compared under, and the worktree is not touched in between. "Already in the base" still ends the loop as before.

Other changes: landing's verification (first verify, reruns, bisect, re-test, `post_land`) takes a broker lease for the test step's or the suites' `uses`, ranked as the batch's first task, outside every timeout, so it is seen by the CPU gate and queues behind steps that asked first. The self-deadlock is gone because no lock is held while suites run.

Trust: this changes when a lock is held, not what must pass. It touches `crates/landing/**` and `crates/daemon/src/landing.rs` (guards); `suites.rs` is untouched.
