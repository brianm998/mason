# 0064: A landed task's build directory is removed by MASON, from a named setting

Status: accepted (s78)

## Context
The build directory is outside the worktree on purpose (a shared one broke parallel tasks), and only the project's `test` command knows where it is. 29 landed tasks' directories filled a disk.

## Decision
- An optional project setting `task_cache` names the directory, with `{task}` required so no two tasks share one. MASON does not guess it from the test command.
- It is removed when the task lands, and only then: canceled and failed tasks can be revived.
- Removal runs on its own thread after `cleanup()` drops the repo lock. A refused or failed removal is logged and never fails the landing.
- Refused: empty expansion, relative path, `..`, home or a parent of it, the project, the worktrees/landing/data directories or a parent of any, a symlink.
- The sweep removes landed tasks' leftovers every pass, skipping tasks with a running job. It files no bug: a restart between landing and removal is ordinary.

## Consequences
Without the setting nothing changes. The unset-variable case expands to nothing and is refused rather than becoming `/{task}`-like paths.
