# 0769: a host is Running only if its pid is still the recorded process

Decision: `status_of` compares the pid's start time with `host_start` and rejects zombies, instead of only probing the pid. Zombies are reaped by a delayed two-pass `waitpid` on specific pids, not `waitpid(-1)`. Rejected: `waitpid(-1)` or a SIGCHLD handler (steals exit statuses from `Child::wait` threads); a persistent "handled hosts" record on disk (a Lost host is skipped by the sweep anyway, the in-memory once-per-id set only covers a restart-free window).
