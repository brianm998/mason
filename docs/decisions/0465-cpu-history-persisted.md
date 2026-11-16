# 0465: the CPU history's minute buckets are saved to disk; batch and debt hosts get their own attribution

**Decision.** This replaces the "not persisted" part of 0321. The daemon writes the minute buckets (the 24 h ring, plus the open minute) to `cpu-history.txt` in the data dir every 60 readings (5 minutes) and when it is told to stop, and loads the file on start, dropping buckets older than 24 h. The 5 s fine ring stays in memory. A crash loses at most five minutes.

Attribution by step host id (`cputasks::host_bucket`): `t<task>r<run>-...` (steps and checks) and `ledger<task>-...` go to the task, `debt<N>-...` to task N, and `b<batch>-...` (floor, retest, baseline builds) to a new "batch #N" share (`kind` "batch", `task` = the batch number). What no running host's tree or path claims stays "other".

**Why.** The owner's 24 h performance review found no history (18 restarts a day) and 61-68% of the machine under "other" while batch and debt builds ran.

**Not verified.** Check hosts (`t445r3063-check-...`) already parse as task hosts, so why #445 showed 5.6% while a rustc ran under it was not found; it is not a naming gap. Candidates: the host not `Running` in the registry or `child_pid` 0 at the sampling moment. The monitors draw the new "batch" kind with the "other" colour and the name "batch" (their fallback); a colour token and name belong in `clients/features.toml` and the clients, which are in `PROTECTED` and wait for the owner. Nothing was run against a real machine.
