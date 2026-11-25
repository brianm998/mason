# 1064: the exit-script watcher must not hold the output pipe

## Cause
The fallback in scripts/run-exit-scripts.sh (no timeout/gtimeout, i.e. macOS) ran a watcher subshell whose `sleep` inherited the pipe into `tee`. After a script passed, only the subshell was killed; the orphan sleep kept the pipe open for up to the limit.

## Decision
Decisions 1063, 1065, 1066 and 1068 landed first and carry the fix: the watcher keeps no stdio, its sleep is killed, each script's process group is reaped, and the macOS job limit is 90 minutes so the 3600 s per-script limit fires before the job is cancelled. This task keeps that code and adds:
- MASON_EXIT_FALLBACK=1, which forces the watcher where timeout(1) exists, so the fallback can be tested there too.
- A comment in ci.yml saying why the macOS job limit is 90.

It replaces 1068's "keep the 60-minute job timeout", as 1066 did: with a 3600 s limit inside a 60-minute job, a hung script is never reported by name.

## Rejected
- Raising the job timeout alone: does not stop the pipe hang.
- Lowering the 3600 s limit: changes a gate.
- Requiring Homebrew coreutils timeout on the runner: assumes a tool it may lack.

## Trust
scripts/run-exit-scripts.sh, scripts/exit/**, .github/workflows/** need owner approval.
