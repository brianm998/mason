# 1064: the exit-script watcher must not hold the output pipe

## Cause
The fallback in scripts/run-exit-scripts.sh (no timeout/gtimeout, i.e. macOS) ran a watcher subshell whose `sleep` inherited the pipe into `tee`. After a script passed, only the subshell was killed; the orphan sleep kept the pipe open for up to the limit.

## Decision
The watcher runs with stdio on /dev/null, its sleep is killed when the script ends, and the script runs in its own process group (set -m) so the limit kills all descendants. exit-scripts-macos gets timeout-minutes 90 like exit-scripts, so the 3600 s per-script limit fires before the job is cancelled. MASON_EXIT_FALLBACK=1 forces the fallback for tests.

## Rejected
- Raising the job timeout alone: does not stop the pipe hang.
- Lowering the 3600 s limit: changes a gate.
- Requiring Homebrew coreutils timeout on the runner: assumes a tool it may lack.

## Trust
scripts/run-exit-scripts.sh, scripts/exit/**, .github/workflows/** need owner approval.
