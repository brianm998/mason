# 1298: A recovery session with no task worktree reads a checkout of the base

**Decision.** A task that failed before its worktree was made is diagnosed in a detached checkout of the base, as a cause diagnosis of several tasks already is. The task's branch is not created for it: a session that only reads leaves no ref behind, and the next develop step makes the branch from the base of its own day.

**A repair is refused there.** There is no branch to change, so the task is handed off.
