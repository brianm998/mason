# 1067: the exit-script watcher must not hold the output pipe

Status: accepted (#1088)

Without timeout(1) (macOS runners), `run-exit-scripts.sh` starts a watcher subshell that sleeps for the
limit. It inherited the stdout piped into `tee`, and killing the subshell left its `sleep` alive, so `tee`
saw no EOF until the limit (3600 s in CI) passed. The job then ran out its 60 minutes after one script.

Decision: the watcher's output goes to /dev/null and its sleep is killed when the script ends. Limits and
job timeouts are unchanged.
