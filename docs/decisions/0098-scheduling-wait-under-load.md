# 0098: wait on state under one deadline, not a fixed 15 s

Decision: `wait_explained` polls `explain_task` until it returns Some, with a single 90 s deadline (about 4x the time the test takes alone). The timeout panic keeps the task state and event dump.

Rejected: raising the number on the old 15 s clock. It would still be a guess tied to one machine's speed and would leave two competing clocks (15 s and the outer 40 s) to keep in step.

Follow-up if needed: the 20/30/60 s waits elsewhere in `scheduling.rs`.
