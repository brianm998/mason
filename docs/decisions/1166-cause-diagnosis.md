# 1166: one diagnosis per cause outside the code, with a fix-now and a fix-later

Decision: a failure that is not the task's own code (a landing failure of any class, a second outside
failure with the same signature, a `ci-stuck` batch, an `autoretry` burst) is diagnosed once per cause
by a read-only session that is given the facts MASON already has. Its card pairs a fix-now action with
a fix-later one; `land_now`, `file_task`, `wait_for` and `open_session` always wait for the owner.
Why: task #1003 timed out twice on a `trd` CI job that was hung on the base; the blind auto-retry
started another 2 h wait, and the owner had to find the cause and file the ticket by hand.

- The automatic retry waits for the diagnosis (at most 900 s) and for an open card that does not
  recommend a lone `retry`; a session without a verdict, or a recommended `retry`, lets it go.
- At most 4 cause-keyed diagnoses per project per day, on top of #1044's per-failure budget.
- `wait_for` is a daemon-side wait, not an engine blocker (the engine takes blockers only on
  proposed or queued tasks).
- Protected files touched: `contract.rs` (only stricter checks), `attention.rs` (a Landing task is
  served a cause card; answering stays owner-only). `landing.rs` and `task.rs` are untouched.
