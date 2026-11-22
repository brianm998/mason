# 0905: monitor-compose (ubuntu-latest) red, run 45005421555

## Decision

Change nothing: no edits to tests, deadlines, `ci.yml` or `s13.sh`. The supplied tail (commit 38a226d26, develop) is post-job Gradle cache cleanup only, so the failing step is unknown. Per decisions 0620, 0630, 0649, 0672, 0690 and 0707 (stages s627, s637, s656, s679, s697, s714), a fix chosen without the failed-step log would repeat s572: raised numbers that did not hold.

## Classification from the full failed-step log

1. `FAILED: <class> <name>`: a test failed. Make that test wait on a Signal rather than a sleep, as s598 did.
2. `timed out waiting for task ...`: a step-4 real-daemon wait expired. Fix the path that does not signal, not the number.
3. A Gradle or setup-gradle error with no testcase: record it and file a proposal.

## Hypotheses to check, not causes

- The 30-minute job timeout (`timeout-minutes: 30`, no rust-cache, so s13.sh's cold `cargo build -q --workspace --bins` runs before Gradle). A job cancelled at its limit shows neither a FAILED nor a timed-out line; check the cargo build step's end time.
- The step-4 deadlines in s13.sh (60 s, 120 s, 30 s) that print `timed out waiting for task`.

## Recommendation

Attach the failed-step log, not the tail, to base-branch CI reds.
