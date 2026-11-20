# 0726: Landing, baselines and debts follow the global switch; bisect halves run side by side

A landing batch spans tasks, so no one task's `run_on` can decide it. Every run no task owns (batch and lint tiers, reruns, bisect runs, the build floor, baselines at the merge base, runs for debts) asks the same placement as a task's run, with no task: the global switch, then the suite's `remote = false`, device, `exclusive` and setup rules (0715). With the switch off nothing changes.

Only the place and the order of independent runs change; the same parser, minimum and verdict logic decide. A red split's two halves are asked together (`plan::pin_paired`) only when every suite the split can run goes to GitHub; each half is assembled in its own worktree, so the culprit is the one the one-at-a-time search finds, and a local split keeps the old order.

Unchanged: promotion, pushing the base, the freeze and `direct-ci` candidate refs (the project's own CI).

Decided against: running on the bare base commit with no new push. The spec rides in the snapshot (`.mason/run.json`), so each run pushes its own overlay commit on top of the base.
