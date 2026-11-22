# 0797: A batch runs its narrowed tests; the scheduled full test is the backstop

Status: proposed, waits for the owner (touches `crates/daemon/src/landing.rs`, `crates/landing/**`, `.mason/**`).

## Context

Landing ran the whole `cargo-workspace` suite once per batch (decision 0539), 22-35 min, although each task's test step had already run the tests its change affects. The owner wants full runs less often; #782 added a scheduled full test with a record of its verdicts.

## Decision

- A `batch` tier with `narrow = true` runs each suite's `dev_command` over the union diff plus the smoke set, and the whole suite only when the pick is all.
- It is refused unless the project has a full-test schedule with a trigger, so the whole suite always runs somewhere.
- While the last full test is red, missing, or older than `full_test_max_age` (default 36 h) with untested commits since, batches run the whole suite until a full test is green. The note and `/landing` say why.
- The red ladder is unchanged and works on the narrowed run, because each rung runs through the same function.

## Consequences

A regression in a test no narrowing picks now lands, and the next full test finds it; #782's range since the last green and blame candidates handle that. This trades some landing safety for 20+ minutes of machine time per batch. It is not enabled for MASON itself in this change.
