# 1067: the exit-script watcher must not hold the output pipe

Status: accepted (#1088)

Without timeout(1) (macOS runners), `run-exit-scripts.sh` starts a watcher subshell that sleeps for the
limit. It inherited the stdout piped into `tee`, and killing the subshell left its `sleep` alive, so `tee`
saw no EOF until the limit (3600 s in CI) passed. The job then ran out its 60 minutes after one script.

Decision: the watcher's output goes to /dev/null. Killing its sleep when the script ends is decision 1068,
which landed first; the redirect also covers a sleep forked after that kill. Limits and job timeouts are
unchanged.
