# 0585: Make the exit-script runner name its failures

The shard-3 culprit in run 44298225037 is still unknown: the log tail given to the task lacks the `FAILED: scripts/exit/sNNN.sh` line. Reading the full job log (or the next red run, which now ends with `FAILED scripts:`) settles it; a separate fix then goes to that script.

Decision: only add a summary line. The exit status and shard selection stay as they were.
