# 0907: shard 3's red scripts fixed at their own fixtures

Cause: s546 failed on a unit-test fixture that never registered its `tpl` target; s669's temp repo lacked the files the lint now reads (check-workflows.sh, workflow files, a package manifest). Neither was Linux-specific.

Decision: fix the fixtures, keep every assertion, and leave s09, s40, s422 and s777 unchanged because no failure line was seen for them. s914.sh reruns all six and reports every failure together so the next red shows its cause.
