# 1134: what the pipeline removes from its own worktrees

## Context

On 2026-11-25 MASON's worktrees took 260 GB, 188 GB of it canceled tasks, and the disk floor stopped all work. Only landed and branch-deleted tasks were cleaned up.

## Decision

- A canceled task with its branch kept loses its worktree after 24 h, or at once when the branch is already in the base. A failed task loses it after 72 h, never early: its branch is often in the base only because nothing was committed, and the diagnosis reads the worktree. A canceled self-change that was held and not approved counts as failed. A proposal rejected before it started has no worktree. There is no separate rejected state.
- A branch is never deleted by this. A worktree is not removed while a job, open step run, step host, landing batch or owner session uses it, when its branch is missing, when it has uncommitted changes, or when HEAD is not on the task's branch.
- Build output is only a directory git ignores that is named `target`, `build` or `.build` or listed by the project, so nothing tracked or uncommitted can be in it.
- Disk pressure is less than twice `disk.floor_gb` free on the data directory's disk. The floor itself is unchanged.
- Removal renames into `<data dir>/trash` under the repository lock and deletes outside it, one at a time at background priority, so git work is not held for the minutes a large delete takes.
- Landing worktrees are disposable (each batch checks out afresh), so they have no dirty check.
- `mason disk clean` removes, so it is the owner's; the dry run and the report are open.

## Consequences

- The first sweep after this lands removes the old canceled and failed worktrees that are clean.
- A build after a trim is cold.
- A landing worktree beyond the cap of two is rebuilt cold the next time three batches run at once; raise `landing_scratch_max` if that happens often.
- A squash-merged branch is not "in the base", so it waits the 24 h.
