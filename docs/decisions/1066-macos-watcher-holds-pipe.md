# 1066: macOS watcher holds the pipe

## Context
Without timeout(1), the runner's watcher subshell inherited the pipe to `tee` and its `sleep` outlived the script, stalling each macOS shard until the limit (3600 s).

## Decision
The watcher's stdio is detached and its children are killed when the script ends (landed on develop by c1e58b9b7 and 24a4fd4e6 while this task ran). This task raises the macOS job to 90 minutes like the Linux one.

## Consequences
A macOS script over the limit now fails by name; a finished script is no longer delayed. The change touches protected paths (scripts/exit/**, .github/workflows/**) and needs owner approval.
