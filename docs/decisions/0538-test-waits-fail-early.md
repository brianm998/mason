# 0538: A test wait fails when it can no longer succeed, not by time

Decision: the 900 s floor stays (`MIN_WAIT_SECS` is not lowered, no wait gets shorter by time). Waits instead return early when the helper can see the awaited outcome is impossible: the task is in a terminal state other than the wanted one (or one the caller declares settled), or a scratch daemon of the test has exited. Early exits only turn a would-be timeout into a failure with the reason; no test asserts anything different.

Why not lower the floor: slow step-host launches on a busy or syspolicyd-stalled Mac are not failures.
Daemon-exit detection: `ps` reports a zombie; only an exited, unreaped daemon counts, so tests that stop and reap their daemon are unaffected, and `--managed` take-over (exec, same pid) is not an exit.
Trust: `tests/support/` is not protected; `tests/fixtures/**` untouched.
