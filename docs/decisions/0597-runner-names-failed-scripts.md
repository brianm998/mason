# 0597: the runner names failed scripts in its last line

Failure logs are shown as a tail, and the per-script `FAILED:` line goes to stderr mid-log, so reds like shard 2 (s575) and shard 6 (s604) could not be attributed. The runner now prints `FAILED scripts: <paths>` as its last stdout line when any script fails. Exit status is unchanged and s580 is untouched.

scripts/run-exit-scripts.sh is a protected gate (crates/daemon/src/protect.rs:32), so this edit is held for owner approval.
