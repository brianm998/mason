# 1066: macOS watcher holds the pipe

## Context
Without timeout(1), the runner's watcher subshell inherited the pipe to `tee` and its `sleep` outlived the script, stalling each macOS shard until the limit (3600 s).

## Decision
Detach the watcher's stdio, kill its children when the script ends, and raise the macOS job to 90 minutes like the Linux one.

## Consequences
A macOS script over the limit now fails by name; a finished script is no longer delayed. The change touches protected paths (scripts/run-exit-scripts.sh, scripts/exit/**, .github/workflows/**) and needs owner approval.
