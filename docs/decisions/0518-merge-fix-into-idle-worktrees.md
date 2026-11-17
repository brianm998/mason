# 0518: merge a landed fix into the idle worktrees that hit the red

**Decision.** While a develop agent's `run_check` is queued, if a known-red test that its last check failed has left the known-red ledger, MASON merges the base into its worktree (a signed-off merge commit) before the check runs, and tells the agent on the result. It does so only when the worktree has no uncommitted tracked change and no merge in progress, and `git merge-tree` finds the merge clean. Otherwise nothing is touched.

**Why.** A queued agent cannot merge by itself, and its check would fail on a bug the base has fixed. Ten of fifteen worktrees were stale on 2026-11-16 after a fix.

**Why polling the ledger.** A ledger entry leaves only when a run on the base executed and passed the test (`lift_proven`), which is the moment the red turns green there. That code is in `suites.rs`, which is protected (verdict logic); each waiting check polls the ledger instead, so no protected file changes.

**Why only idle.** A queued check is the one time the agent is certainly not editing. A running agent is never touched.

**Trust.** `jobs.rs`, `fixsync.rs` and `basefold.rs` are not in `PROTECTED`. The new exit script `scripts/exit/s525.sh` is the numbered one MASON gave this task.
