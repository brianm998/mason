# 0656: the fast-tier landing test forms batches by size, not by time

**Decision.** The test admits its first task with `max_size = 1` and a long window, writes the skip debt before admission, and waits for the batch to finish before creating the next task.

**Why.** A 2 s window depends on the wall clock, and the next task's git worktree work could overlap the previous batch's teardown. Nothing in production changes; this is a test-only change.
