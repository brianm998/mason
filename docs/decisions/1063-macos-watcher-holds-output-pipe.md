# 1063: the fallback watcher must not hold the output pipe

The runner pipes each script through `tee`. Without timeout(1) the watcher's `sleep` inherited that pipe and outlived the killed watcher, so `tee` waited for the whole limit. We redirect the watcher's stdio to /dev/null and kill its child with `pkill -P` before the watcher. We did not lower the limit: the repeat scripts need it. The macOS job limit matches the ubuntu job's 90 minutes.
