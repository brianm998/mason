# 0620: no change for the monitor-compose ubuntu red until its failing step is read

Decision: nothing but this record and s627 changes. The only log supplied for run 44399898483 (commit 88ba43ce) ends in the post-job step, for example `Saved cache entry with key gradle-home-v1|Linux-X64|monitor-compose[...]-88ba43ce...`; it names no failing step, testcase or deadline, so no cause can be quoted. Changing a test or a deadline on a guess would repeat s572, which raised numbers and did not hold.

To classify it, read the s13.sh step of the full job log for: `FAILED:` (a Gradle testcase: make that test wait on a `Signal`, as s598 did), `timed out waiting` (a real-daemon or socket deadline: fix the path that does not signal the event, not the number), or a Gradle/setup-gradle error with no testcase (infrastructure: record it and file a proposal, leave the tests).

Not run here: scripts/exit/s13.sh and `:core:test` (no Linux host, no network). No assertion was removed or weakened, and the README Status block is untouched.
