# 1170: no change for monitor-compose ubuntu red run 45883735030 until its failing step is read

## Context

`CI/monitor-compose (ubuntu-latest)` was red in run 45883735030 (commit 0f7483b0c, `develop`), on the first attempt and again when its failed jobs were rerun. The scheduled `full-ci` run filed it. No earlier green full test is on record, so there is no commit range.

The log supplied is the tail of the rerun: post-job Gradle cache cleanup, `Cache hit occurred on the cache key ..., not saving cache`, git cleanup and the `Node.js 20 is deprecated` warning. It names no failing step, testcase or deadline. Earlier reds with tails like this are in decisions 0620, 0630, 0649, 0672, 0690, 0707, 0905 and 1087 (stage s1098); 1019 is the one whose full log could be classified.

## Decision

No test, deadline, `ci.yml`, `scripts/exit/s13.sh` or `Connection.kt`/`Tool.kt` change is made until the failed-step log is read. Only this record, stage s1180 and its exit test are added.

Read it with `gh run view 45883735030 --log-failed` and classify it:

- FAILED testcase, `FAILED: <class> <name> - <message>`: make that test wait on a `Signal`, as s598 did.
- Timed-out deadline, `timed out waiting for task ...`: fix the path that does not signal the event, not the number.
- Gradle or setup-gradle error with no testcase: record it and file a proposal, leave the tests.

A step 2 `conformance` failure is none of these three: s13.sh then prints the per-checkpoint id differences (1019), and those are the evidence.

## Observations, not a diagnosis

- The cache cleanup removes files not accessed since 22:40:26 UTC and the last line is at 22:43:02, about 2.5 minutes apart. That is weak evidence against the 30-minute `timeout-minutes` firing, but check the step timestamps.

## Not run here

The failed-step log was not read (no network). `scripts/exit/s13.sh` and Gradle were not run.

## Trust

No protected path is touched.
