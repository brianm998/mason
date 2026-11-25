# 1067: the exit-script watcher must not hold the output pipe

Status: accepted (#1088)

Without timeout(1) (macOS runners), `run-exit-scripts.sh` starts a watcher subshell that sleeps for the
limit. It inherited the stdout piped into `tee`, and killing the subshell left its `sleep` alive, so `tee`
saw no EOF until the limit (3600 s in CI) passed. The job then ran out its 60 minutes after one script.

Decision: the watcher's stdio goes to /dev/null and its `sleep` is killed when the script ends. Both landed
first, as decisions 1068 (the kill) and 1065 (the redirect). This task adds only its exit test, which also
checks that a failing script is still reported on the fallback path and that no `sleep` is left behind.
Limits and job timeouts are unchanged.
