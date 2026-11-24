# 1065: kill the watcher's sleep and detach its stdio

## Context

CI/exit-scripts-macos (1) was cut off at its limit (run 45361042086, commit 0de921ce). The macOS runner has no timeout(1), so the runner's watcher subshell sleeps for the limit. After a passing script only the subshell was killed; the orphaned `sleep` kept the pipe to `tee` open for up to the limit.

## Decision

Redirect the watcher's stdio to /dev/null and kill its children before the watcher itself.

## Rejected

Installing coreutils timeout on the macOS runner: it would hide the bug on macOS while the fallback path stayed broken for anyone else without timeout.
