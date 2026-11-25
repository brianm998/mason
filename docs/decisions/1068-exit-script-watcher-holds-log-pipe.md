# 1068: the exit-script watcher must not outlive its script

## Context

Without timeout(1), the runner's watcher subshell sleeps for the limit. Killing the subshell leaves the sleep running, and it holds the pipe into `tee`, so the runner blocks until the limit expires (run 45361042086).

## Decision

Kill the watcher's children before the watcher. Keep the 60-minute job timeout and the 3600 s limit.

## Consequences

macOS legs finish when their scripts do. A script over the limit is still killed and reported as before. `scripts/run-exit-scripts.sh` is a protected trust path, so the change needs owner approval.
