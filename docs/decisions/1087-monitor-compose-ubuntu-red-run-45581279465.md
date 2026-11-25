# 1087: no change for monitor-compose ubuntu red run 45581279465 until its failing step is read

## Context

`CI/monitor-compose (ubuntu-latest)` was red in run 45581279465 (commit ff4fae6a6, `develop`), on the first attempt and again when its failed jobs were rerun. The scheduled `full-ci` run filed it. No earlier green full test is on record, so there is no commit range.

The log supplied is the tail of the rerun: post-job Gradle cache cleanup, `Cache hit occurred on the cache key ..., not saving cache`, git cleanup and the `Node.js 20 is deprecated` warning. It names no failing step, testcase or deadline. The warning is a notice, not shown to be the failure. Earlier reds with tails like this are in decisions 0620, 0630, 0649, 0672, 0690, 0707 and 0905; 1019 is the one whose full log could be classified.

## Decision

No test, deadline, `ci.yml`, `scripts/exit/s13.sh` or `Connection.kt`/`Tool.kt` change is made until the failed-step log is read. Only this record, stage s1098 and its exit test are added.

Read it with `gh run view 45581279465 --log-failed` and classify it:

- FAILED testcase, `FAILED: <class> <name> - <message>`: make that test wait on a `Signal`, as s598 did.
- Timed-out deadline, `timed out waiting for task ...`: fix the path that does not signal the event, not the number.
- Gradle or setup-gradle error with no testcase: record it and file a proposal, leave the tests.

A step 2 `conformance` failure is none of these three: s13.sh then prints the per-checkpoint id differences (1019), and those are the evidence. A capturer that gives up prints `monitor-tool: timed out waiting for ...` (Compose) or `error: ...` (Rust).

## Observations, not a diagnosis

- The cache cleanup removes files not accessed since 11:52:15 and the job's last line is at 11:54:58. If that cutoff is the job's build start, s13.sh ran for under three minutes, so the 30-minute `timeout-minutes` is probably not the cause. This is unconfirmed: check the step timestamps.
- Decision 1057 (stage s1068) says to read the `quiet exit` lines on a conformance red. At ff4fae6a6 only the Rust capturer still prints one (`settle: quiet exit, ...`). Compose's lines are gone: the merge 524955f38, just before ff4fae6a6, left `Tool.kt` with the signal-based settle of decision 1026 and without 1057's change. Their absence from a Compose log therefore says nothing. Whether that merge is connected to this red is an untested hypothesis. It is filed as a separate proposal.

## Not run here

The failed-step log was not read (no network). `scripts/exit/s13.sh` and `:core:test` were not run (no Linux host).

## Trust

No protected path is touched.
