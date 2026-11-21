# 0780: Credit the whole GitHub wait of a run_check

Status: proposed (task #799), needs the owner's approval: it changes budget logic (`deadline.rs`, `jobs.rs`).

Decision: a `run_check` that goes to GitHub has all of its wait credited to the develop step's deadline: the wait for a pool unit, for a runner to pick the job up, and the run itself. It is added as it goes (as the slot-queue credit is), so a daemon restart keeps it, and it is shown apart in the timeout note as "waiting on GitHub". The task suggested crediting only the run's time beyond its usual time on this Mac; I chose the whole run because it is simpler, needs no per-suite history, and the agent is idle for all of it. The cost: a check that runs far longer than usual on GitHub also extends the deadline. The check's own limit (`spec.limit`, up to 60 min) still ends a runaway run, and the hard cap (budget × 2 plus credit) is unchanged, so a stuck agent still ends.

The timeout note counts from the step's recorded launch (`step_runs.started_at`), not from this daemon process, and says when the daemon started after the launch.

Alternatives: credit only pool and pickup (leaves 60 min runs charged, which is what ran seven steps out of time); credit run time beyond the last run on this Mac (needs history that GitHub runs do not write).
