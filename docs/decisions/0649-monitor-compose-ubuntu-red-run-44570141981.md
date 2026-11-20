# 0649: no change for monitor-compose ubuntu red run 44570141981 until its failing step is read

Decision: nothing but this record and s656 changes. The log supplied for run 44570141981 (commit e49c21c4) ends in post-job Gradle cache cleanup and `Saved cache entry ...`; it names no failing step, testcase or deadline. This repeats decisions 0620 and 0630 and stages s627 and s637. Guessing a fix or raising a deadline would repeat s572.

Classify it from the s13.sh step of the full job log, read with `gh run view 44570141981 --log-failed` (or the job log in the Actions UI):
- `FAILED: <class> <name> - <message>` (a Gradle testcase): make that test wait on a `Signal`, as s598 did.
- `timed out waiting for task ...` (a deadline): fix the path that does not signal the event, not the number.
- A Gradle or setup-gradle error with no testcase: infrastructure; record it and file a proposal, leave the tests.

Recommendation: the pipeline should attach the failed-step log, not the tail, to base-branch CI reds.

Not run here: scripts/exit/s13.sh and `:core:test` (no Linux host, no network). No assertion, timeout or sleep was changed, and README.md is untouched.
