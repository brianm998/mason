# 0179: A takeover waits for running landing batches

A takeover exec'd at once, so the new daemon abandoned every other project's running batch and its tasks waited a whole window again (star batches #121, #124, #126 on 2026-11-12).

**Decision.** Before `take_over`, the update waits until no landing batch is running, with a daemon-wide drain flag that stops new batches (queued tasks keep their place). The wait is capped by `takeover_wait_s` (default 3600); at the cap the takeover goes anyway and says which batches it abandons. `mason update --now` skips the wait. A failed takeover clears the flag. Both callers (`POST /update`, branch switch) use `update::drain_and_take_over`.

**Queue time.** `landing_submitted(task, run, at_ms)` keeps the first submission of a landing run; after a restart `ready_at` counts from it.

**Trust.** Edits `update.rs` and `landing.rs` (protected): the flag only holds new batches back and never changes a verdict, a ladder step or a gate.
