# 0672: no change for monitor-compose ubuntu red run 44615387131 until its failing step is read

Decision: nothing but this record and s679 changes. The log supplied for run 44615387131 (commit 190be04d) ends in post-job Gradle cache cleanup and `Saved cache entry ...`; it names no failing step, testcase or deadline. This repeats runs 44399898483, 44453947626 and 44570141981, decisions 0620, 0630 and 0649, and stages s627, s637 and s656. Guessing a fix or raising a deadline would repeat s572.

Classify it from the s13.sh step of the full job log, read with `gh run view 44615387131 --log-failed` (or the job log in the Actions UI):
- `FAILED: <class> <name> - <message>` (a Gradle testcase): make that test wait on a `Signal`, as s598 did.
- `timed out waiting for task ...` (a deadline): fix the path that does not signal the event, not the number.
- A Gradle or setup-gradle error with no testcase: infrastructure; record it and file a proposal, leave the tests.

Recommendation: the pipeline should attach the failed-step log, not the tail, to base-branch CI reds.

Not run here: scripts/exit/s13.sh and `:core:test` (no Linux host, no network). No assertion, timeout or sleep was changed, and README.md and all tests are untouched.
