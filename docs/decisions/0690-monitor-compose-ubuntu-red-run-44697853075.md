# 0690: no change for monitor-compose ubuntu red run 44697853075 until its failing step is read

Decision: nothing changes beyond this record and s697. The log supplied for run 44697853075 (commit 10d6cb29) ends in post-job Gradle cache cleanup and `Saved cache entry ...`; it names no failing step, testcase or deadline. This repeats runs 44399898483, 44453947626, 44570141981 and 44615387131, decisions 0620, 0630, 0649 and 0672, and stages s627, s637, s656 and s679. Guessing a fix or raising a deadline would repeat s572.

Classify it from the s13.sh step of the full job log, read with `gh run view 44697853075 --log-failed`. There are three classifications:
- FAILED testcase, `FAILED: <class> <name> - <message>`: make that test wait on a `Signal`, as s598 did.
- Timed-out deadline, `timed out waiting for task ...`: fix the path that does not signal the event, not the number.
- Infrastructure error with no testcase (a Gradle or setup-gradle error): record it and file a proposal, leave the tests.

Recommendation: the pipeline should attach the failed-step log, not the tail, to base-branch CI reds.

Not run here: the full failed-step log was not read, and scripts/exit/s13.sh and `:core:test` were not run (no Linux host, no network). No assertion, timeout or sleep was changed, and README.md, ci.yml and all tests are untouched.
