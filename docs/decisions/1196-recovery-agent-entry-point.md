# 1196: One recovery entry point for every failed or stuck task

Status: accepted, waiting for the owner's approval of the automatic actions below

Every `StepFailed` goes through `Daemon::emit` or `emit_for_run`, so the recovery hook sits there (`recover_failed`), not at the call sites. A guard test lists the files that build a `StepFailed` and fails on a new one.

The session is #1044's diagnosis session. It gains `repair` (a develop-contract session in the task's worktree, then a retry from `to_step`), the task-aimed `unblock` and `release_holder`, and the verdict `handoff`. A session that cannot answer leaves a daemon-written brief, stored on the task, with an alert of kind `recovery_handoff`.

New automatic actions that need the owner's approval (they apply a recommended option without the owner): a `repair`, and `unblock` / `release_holder` aimed at one task. The owner asked for them on 2026-11-24: "just fix it itself if possible and return the task to the proper place in the pipeline". The limits: a repair is applied by itself once per failure signature, so a second failure with the same signature gets a card; an owner's rejection is never applied by itself; the repair runs under the develop step's guards (protected paths wait for the owner, no weakening edits, `run_check`), pushes and lands nothing, and the task is tested and reviewed again from the step it returns to. Budgets are #1044's: a recovery session counts as a diagnosis, and a spent budget goes to a hand-off.

Protected files touched: `crates/adapter-api/src/contract.rs` (new action and verdict shapes; stricter, not looser) and `crates/core/src/notify.rs` is not listed as protected but gains the alert kind. `attention.rs`, `task.rs` and `prompts.rs` are unchanged: the brief is fenced in `recovery.rs`.

Differences from the task text: the alert kind is `recovery_handoff` (the other kinds use underscores); `recovery_brief` is a table; the repair is a second session rather than the diagnosis session writing; a stuck task's card is a hand-off because a card cannot be answered on a task that has not failed.
