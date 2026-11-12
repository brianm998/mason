# 0192: A retry names the failure it saw, and does not restart a running step by accident

Status: accepted (s197)

**Context.** #171 got two retries one second apart. The second restarted the step the first had just started; the agent the first started was killed (status 143) and counted as a develop failure.

**Decision.**
- `POST /tasks/{id}/retry` takes an optional `seen`, the step run the client showed (`last_run` in the task JSON and `TaskRow.last_run`). If the task's latest run differs, the answer is 409 "already retried (run R is running)". The check and the `Retried` event happen under one lock.
- A task in a step state (running) is refused with 409 unless `restart: true` (`--restart`), whether or not `seen` is given. Failed and needs-attention tasks are retried as before.
- The CLI and both monitors send `seen`. The CLI reads `last_run` just before it posts, so it protects against two racing CLI calls, not against a person who looked long ago.

**Does anything rely on the old behaviour?** In this repository, only tests that retry failed tasks; none retries a running one. A script outside it that retried a running step to restart it now needs `--restart`; I found none but cannot see the owner's scripts. The blocker-link "retry" buttons send no `seen` and act on failed tasks.

**Not verified.** No real daemon run with two simultaneous clients.
