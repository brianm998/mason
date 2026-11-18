# 0552: Seed a new task's build directory from the landing area's warm build

Status: built, **off by default**; not yet measured on the real queue.

## What

Project key `task_cache_seed = true` (the owner sets it; default off, needs `task_cache`). When
`prepare_worktree_only` has made a task's worktree and its directory does not exist, the daemon
looks in the landing area for `<project>-b<slot>` checkouts that are clean and whose `HEAD` is the
commit the worktree is at. The first one whose build directory (the `task_cache` template with
`{task}` replaced by the checkout's name, the form the project's `test`/`build` use) exists is
cloned with `cp -c -R -p` (APFS copy-on-write, mtimes kept) into a sibling `.<n>.seeding` and
renamed into place. Not macOS, a different commit, a dirty checkout, an existing directory or a
failed clone: nothing is made (a failure is logged, the partial copy removed) and the task builds
cold, as before.

## Why mtimes and the commit matter

Cargo calls a workspace unit fresh when its dep-info is newer than every source file it lists.
The task's worktree is checked out after the landing build was made, so every source there is
newer than every cloned artifact (`-p` keeps the artifacts' old mtimes): each workspace crate is
rebuilt, incrementally from the cloned `incremental/` cache, and registry dependencies (whose
sources live in `~/.cargo`, unchanged) are reused. A clone with fresh mtimes (plain `cp -R`) or
from a build newer than the checkout would make stale workspace artifacts look fresh, so only a
build of the same commit is used, and `-p` is not optional.

## Measurement (to be filled from the real queue)

Three new tasks with the key on, three with it off: first `run_check` build wall and CPU time
(from `t/<task>/` check logs), disk used after it, machine load. Spike (decision 0473 variant A):
cold about 600 s wall / 2,700 CPU s; incremental after a one-line daemon edit 285 s / 940 CPU s.

| task | seeded | wall s | CPU s | disk after | load |
|------|--------|--------|-------|------------|------|
| (none measured) | | | | | |

## Recommendation

Leave `task_cache_seed` off until the six rows above are filled in; switch it on only if the
seeded builds are clearly cheaper. Expected saving from the spike is about half.

## Not verified

- The real-queue measurement: I could not run six tasks here.
- That cargo reuses registry artifacts and fingerprints cloned from a different worktree path;
  workspace crates are rebuilt either way, but this is the part to watch in the rows.
- That the landing build is finished when cloned: a clone during a batch's build copies a
  partial directory (cargo then rebuilds what is incomplete).
- The landing build is assumed to be of the checkout's `HEAD`; if `HEAD` moved after the build,
  the clone is of an older build, which is still safe because a task worktree made now has newer
  source mtimes than any existing artifact.
