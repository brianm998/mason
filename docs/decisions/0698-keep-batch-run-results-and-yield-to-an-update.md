# 0698: keep a landing batch's run results, and let an update take over between runs

Status: accepted (s705). Touches `crates/daemon/update.rs` and `landing.rs`, both in `PROTECTED`: the owner approves it.

## Problem

A self-update waited `takeover_wait_s` (1 h) for a running batch and then abandoned it. A red batch's split search takes hours, so on 2026-11-19 two batches (7.5 h of runs) were thrown away, and the next batch began with a full run again.

## Decision

1. **Results are kept.** Table `batch_runs` (migration 56) is keyed by project, base tip, the batch's tasks as sorted `id@branch-head`, and a filter text (suites, rerun, or the split's failing-test/mapped-suite filter). `Runner::cached_run`/`keep_run` in `landing.rs` read and write it. The batch's first run and its whole rerun reuse a recorded **red** only; a split's subset runs reuse red and green, because a split only blames. A different base tip, a new head on any task, another task set or another filter misses, so a different tree is always verified. The flags a red sets for the ladder (floor red, real CI red, lint files) are kept with it. A run that ended in a service error is not kept. Results older than two days are dropped at start.
2. **Yield at a run boundary.** While the daemon is draining for an update, a batch stops, with nothing lost, at the start of a round, at the start of the ladder, and after a split. Inside a split, the next subset run is not started. Its tasks go back to the queue (the same path as deferred tasks), the batch closes as `yielded` with the note "stopped for an update; its run results are kept...", and `drain_within` waits only for the run in progress. `takeover_wait_s` still caps a single long run.
3. **Progress.** A yielding batch says "waiting to land again (batch #N stopped for an update; its results are kept)" on its members. At start, the batches a dead daemon left have the same line written into their tasks' stored progress before they are abandoned.
4. **Status.** `mason status` prints `update: ...` with the batches an update waits for and how long it has waited (`update::status_line`).

A batch note ends with "reused N run result(s) from an earlier batch over the same base and task heads" when any were used.

## Not verified

- Only the tests named in the stage note were run here. A real restart of `masond` with a red batch was not run; the integration test simulates the takeover with `Landing::set_draining`.
- The stored-progress rewrite for an abandoned batch writes the database rows; whether the monitors show the new line before a task's next progress update was not checked.
- A batch that is mid-run when `Update now` is chosen is still abandoned, as before.
