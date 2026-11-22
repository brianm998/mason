# 0942: A signaled agent CLI is retried up to three times

Stage s949. `settle_all` restarted a CLI that exited 143/137 with no result only once. Repeated
signals from daemon restarts failed the step seven times with the same reason. Decision: allow
`MAX_SIGNALED_RETRIES` = 3 per step run; the failure is still classified as outside the code.
